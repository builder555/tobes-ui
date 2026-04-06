//! TorchBearer spectrometer driver.
//!
//! Protocol (USB-serial, 115200 8N1):
//!   Outgoing frame  →  CC 01 <len:3LE> <type:1> <data> <checksum:1> 0D 0A
//!   Incoming frame  →  CC 81 <len:3LE> <type:1> <data> <checksum:1> 0D 0A
//!
//! GET_DATA response payload (little-endian):
//!   B  – exposure status (0=normal, 1=over, 2=under)
//!   I  – exposure time µs
//!   H  – encoded exponent
//!   I  – serial number
//!   Q  – ex_info
//!   [H]– encoded spectrum (2 bytes per pixel)

use anyhow::{bail, Result};
use serialport::SerialPort;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ExposureStatus {
    Normal,
    Over,
    Under,
}

#[derive(Debug, Clone)]
pub struct Spectrum {
    pub status: ExposureStatus,
    pub exposure_time_ms: f32,
    pub wavelengths: Vec<f32>,
    pub intensities: Vec<f32>,
    pub wavelength_start: u16,
    pub wavelength_end: u16,
}

// ---------------------------------------------------------------------------
// Message types
// ---------------------------------------------------------------------------

#[repr(u8)]
#[allow(dead_code)]
enum MsgType {
    Stop            = 0x04,
    GetDeviceId     = 0x08,
    SetExposureMode = 0x0A,
    GetExposureMode = 0x0B,
    SetExposureValue= 0x0C,
    GetExposureValue= 0x0D,
    GetRange        = 0x0F,
    GetData         = 0x33,
}

// ---------------------------------------------------------------------------
// Frame building
// ---------------------------------------------------------------------------

fn checksum(data: &[u8]) -> u8 {
    data.iter().fold(0u8, |a, &b| a.wrapping_add(b))
}

fn build_frame(msg_type: u8, payload: &[u8]) -> Vec<u8> {
    // total length = 2 (header) + 3 (len) + 1 (type) + payload + 1 (chk) + 2 (CRLF) = 9 + payload
    let total = 9 + payload.len();
    let mut f = Vec::with_capacity(total);
    f.push(0xCC);
    f.push(0x01);
    f.push((total & 0xFF) as u8);
    f.push(((total >> 8) & 0xFF) as u8);
    f.push(((total >> 16) & 0xFF) as u8);
    f.push(msg_type);
    f.extend_from_slice(payload);
    let chk = checksum(&f);
    f.push(chk);
    f.push(0x0D);
    f.push(0x0A);
    f
}

// ---------------------------------------------------------------------------
// Buffered reader
// ---------------------------------------------------------------------------

struct FrameReader {
    port: Box<dyn SerialPort>,
    buf:  Vec<u8>,
}

impl FrameReader {
    fn new(port: Box<dyn SerialPort>) -> Self {
        Self { port, buf: Vec::with_capacity(2048) }
    }

    fn read_frame(&mut self) -> Result<(u8, Vec<u8>)> {
        loop {
            // Top up the buffer with whatever is available
            let mut chunk = [0u8; 512];
            match self.port.read(&mut chunk) {
                Ok(0) => {}
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(e) => return Err(e.into()),
            }

            if let Some(r) = self.try_parse()? {
                return Ok(r);
            }
        }
    }

    fn try_parse(&mut self) -> Result<Option<(u8, Vec<u8>)>> {
        // Sync to CC 81 header
        loop {
            if self.buf.len() < 2 {
                return Ok(None);
            }
            if self.buf[0] == 0xCC && self.buf[1] == 0x81 {
                break;
            }
            self.buf.remove(0);
        }

        if self.buf.len() < 5 {
            return Ok(None);
        }

        let length = (self.buf[2] as usize)
            | ((self.buf[3] as usize) << 8)
            | ((self.buf[4] as usize) << 16);

        if self.buf.len() < length {
            return Ok(None);
        }

        // Validate checksum (covers everything before the checksum byte)
        let expected = checksum(&self.buf[..length - 3]);
        if expected != self.buf[length - 3] {
            // Bad frame – drop the header byte and resync
            self.buf.remove(0);
            return Ok(None);
        }

        if self.buf[length - 2] != 0x0D || self.buf[length - 1] != 0x0A {
            self.buf.remove(0);
            return Ok(None);
        }

        let msg_type = self.buf[5];
        // data sits between [6] and [6 + (length - 9)]
        let data_end = 6 + length - 9;
        let data = self.buf[6..data_end].to_vec();
        self.buf.drain(..length);
        Ok(Some((msg_type, data)))
    }

    fn write(&mut self, frame: &[u8]) -> Result<()> {
        use std::io::Write;
        self.port.write_all(frame)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Spectrum decoding (XOR + exponent obfuscation)
// ---------------------------------------------------------------------------

fn decode_spectrum(
    encoded:          &[u16],
    encoded_exponent: u16,
    exposure_time_ms: f32,
    serial:           u32,
    ex_info:          u64,
) -> Vec<f32> {
    // The device obfuscates data with an XOR key derived from the exposure
    // parameters.  This mirrors the Python _decode_spectrum implementation.

    // Reinterpret the f32 bit pattern as u32 (little-endian)
    let et_le: u32 = exposure_time_ms.to_bits();
    // The same 4 bytes read as big-endian u32
    let et_be: u64 = et_le.swap_bytes() as u64;

    let common: u64 = et_be ^ (ex_info >> 16);

    let key_a = (common
        ^ (((et_le as u64) ^ (serial as u64)) >> 16)
        ^ (serial as u64)
        ^ ex_info) as u16; // truncate = & 0xFFFF

    let key_b = ((common >> 16) ^ (et_le as u64) ^ (serial as u64)) as u16;

    // Byte-swap encoded_exponent (LE→BE), then XOR with the magic constant
    let exponent = encoded_exponent.swap_bytes() ^ 8848;
    let scale = (10.0f64).powi(exponent as i32);

    let mid = encoded.len() / 2;
    encoded
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            let key = if i < mid { key_a } else { key_b };
            ((v ^ key) as f64 / scale) as f32
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Public driver
// ---------------------------------------------------------------------------

pub struct TorchBearer {
    reader:          FrameReader,
    pub wavelength_start: u16,
    pub wavelength_end:   u16,
}

impl TorchBearer {
    /// Open the serial port and query the device wavelength range.
    pub fn open(path: &str) -> Result<Self> {
        let port = serialport::new(path, 115200)
            .timeout(Duration::from_secs(5))
            .open()?;

        let mut tb = Self {
            reader: FrameReader::new(port),
            wavelength_start: 340,
            wavelength_end:   1000,
        };
        tb.query_range()?;
        Ok(tb)
    }

    fn send(&mut self, msg_type: u8, payload: &[u8]) -> Result<()> {
        let frame = build_frame(msg_type, payload);
        self.reader.write(&frame)
    }

    fn recv_type(&mut self, expected: u8) -> Result<Vec<u8>> {
        loop {
            let (t, data) = self.reader.read_frame()?;
            if t == expected {
                return Ok(data);
            }
        }
    }

    fn query_range(&mut self) -> Result<()> {
        self.send(MsgType::GetRange as u8, &[])?;
        let data = self.recv_type(MsgType::GetRange as u8)?;
        if data.len() < 4 {
            bail!("GET_RANGE response too short");
        }
        self.wavelength_start = u16::from_le_bytes([data[0], data[1]]);
        self.wavelength_end   = u16::from_le_bytes([data[2], data[3]]);
        Ok(())
    }

    /// Begin continuous acquisition.
    pub fn start_streaming(&mut self) -> Result<()> {
        self.send(MsgType::GetData as u8, &[])
    }

    /// Stop continuous acquisition.
    pub fn stop_streaming(&mut self) -> Result<()> {
        self.send(MsgType::Stop as u8, &[])
    }

    /// Block until the next spectrum frame arrives and return it decoded.
    pub fn read_spectrum(&mut self) -> Result<Spectrum> {
        loop {
            let (t, data) = self.reader.read_frame()?;
            if t != MsgType::GetData as u8 {
                continue;
            }

            // Payload layout: B I H I Q [H…]
            if data.len() < 19 {
                bail!("GET_DATA payload too short ({}B)", data.len());
            }

            let status_code        = data[0];
            let exposure_time_us   = u32::from_le_bytes(data[1..5].try_into().unwrap());
            let encoded_exponent   = u16::from_le_bytes(data[5..7].try_into().unwrap());
            let serial             = u32::from_le_bytes(data[7..11].try_into().unwrap());
            let ex_info            = u64::from_le_bytes(data[11..19].try_into().unwrap());

            let encoded: Vec<u16> = data[19..]
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();

            let status = match status_code {
                0 => ExposureStatus::Normal,
                1 => ExposureStatus::Over,
                _ => ExposureStatus::Under,
            };

            let exposure_time_ms = exposure_time_us as f32 / 1000.0;
            let intensities = decode_spectrum(
                &encoded,
                encoded_exponent,
                exposure_time_ms,
                serial,
                ex_info,
            );

            let pixel_count = (self.wavelength_end - self.wavelength_start + 1) as usize;
            let n = pixel_count.min(intensities.len());

            let wavelengths: Vec<f32> = (0..n)
                .map(|i| (self.wavelength_start + i as u16) as f32)
                .collect();
            let intensities = intensities[..n].to_vec();

            return Ok(Spectrum {
                status,
                exposure_time_ms,
                wavelengths,
                intensities,
                wavelength_start: self.wavelength_start,
                wavelength_end:   self.wavelength_end,
            });
        }
    }
}
