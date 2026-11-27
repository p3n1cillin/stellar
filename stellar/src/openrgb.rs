//! Minimal native client for the OpenRGB SDK network protocol (v3).

use anyhow::{Result, anyhow, bail};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

const PROTOCOL: u32 = 3;
const REQUEST_CONTROLLER_COUNT: u32 = 0;
const REQUEST_CONTROLLER_DATA: u32 = 1;
const REQUEST_PROTOCOL_VERSION: u32 = 40;
const SET_CLIENT_NAME: u32 = 50;
const RESIZE_ZONE: u32 = 1000;
const UPDATE_LEDS: u32 = 1050;
const SET_CUSTOM_MODE: u32 = 1100;
const UPDATE_MODE: u32 = 1101;

#[derive(Clone, Debug, PartialEq)]
pub struct Mode {
    pub name: String,
    pub value: i32,
    pub flags: u32,
    pub speed_min: u32,
    pub speed_max: u32,
    pub brightness_min: u32,
    pub brightness_max: u32,
    pub colors_min: u32,
    pub colors_max: u32,
    pub speed: u32,
    pub brightness: u32,
    pub direction: u32,
    pub color_mode: u32,
    pub colors: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Zone {
    pub name: String,
    pub kind: i32,
    pub leds_min: u32,
    pub leds_max: u32,
    pub leds_count: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    pub kind: i32,
    pub name: String,
    pub vendor: String,
    pub description: String,
    pub modes: Vec<Mode>,
    pub active_mode: i32,
    pub zones: Vec<Zone>,
    pub led_count: usize,
}

struct Reader<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let s = self.b.get(self.p..self.p + n).ok_or_else(|| anyhow!("truncated OpenRGB packet"))?;
        self.p += n;
        Ok(s)
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into()?))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }
    fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }
    fn str(&mut self) -> Result<String> {
        let n = self.u16()? as usize;
        let s = self.take(n)?;
        Ok(String::from_utf8_lossy(s.strip_suffix(&[0]).unwrap_or(s)).into_owned())
    }
}

pub fn parse_device(b: &[u8]) -> Result<Device> {
    let mut r = Reader { b, p: 0 };
    let _size = r.u32()?;
    let kind = r.i32()?;
    let name = r.str()?;
    let vendor = r.str()?;
    let description = r.str()?;
    let _version = r.str()?;
    let _serial = r.str()?;
    let _location = r.str()?;
    let n_modes = r.u16()?;
    let active_mode = r.i32()?;
    let mut modes = vec![];
    for _ in 0..n_modes {
        let name = r.str()?;
        let value = r.i32()?;
        let flags = r.u32()?;
        let speed_min = r.u32()?;
        let speed_max = r.u32()?;
        let brightness_min = r.u32()?;
        let brightness_max = r.u32()?;
        let colors_min = r.u32()?;
        let colors_max = r.u32()?;
        let speed = r.u32()?;
        let brightness = r.u32()?;
        let direction = r.u32()?;
        let color_mode = r.u32()?;
        let nc = r.u16()?;
        let colors = (0..nc).map(|_| r.u32()).collect::<Result<_>>()?;
        modes.push(Mode {
            name, value, flags, speed_min, speed_max, brightness_min, brightness_max, colors_min, colors_max, speed,
            brightness, direction, color_mode, colors,
        });
    }
    let n_zones = r.u16()?;
    let mut zones = vec![];
    for _ in 0..n_zones {
        let name = r.str()?;
        let kind = r.i32()?;
        let leds_min = r.u32()?;
        let leds_max = r.u32()?;
        let leds_count = r.u32()?;
        let matrix_len = r.u16()? as usize;
        r.take(matrix_len)?;
        zones.push(Zone { name, kind, leds_min, leds_max, leds_count });
    }
    let n_leds = r.u16()? as usize;
    for _ in 0..n_leds {
        r.str()?;
        r.u32()?;
    }
    Ok(Device { kind, name, vendor, description, modes, active_mode, zones, led_count: n_leds })
}

pub fn encode_mode(index: i32, m: &Mode) -> Vec<u8> {
    let mut body = vec![];
    body.extend(index.to_le_bytes());
    let s = |b: &mut Vec<u8>, v: &str| {
        b.extend(((v.len() + 1) as u16).to_le_bytes());
        b.extend(v.as_bytes());
        b.push(0);
    };
    s(&mut body, &m.name);
    for v in [m.value as u32, m.flags, m.speed_min, m.speed_max, m.brightness_min, m.brightness_max, m.colors_min,
        m.colors_max, m.speed, m.brightness, m.direction, m.color_mode]
    {
        body.extend(v.to_le_bytes());
    }
    body.extend((m.colors.len() as u16).to_le_bytes());
    for c in &m.colors {
        body.extend(c.to_le_bytes());
    }
    let mut out = ((body.len() + 4) as u32).to_le_bytes().to_vec();
    out.extend(body);
    out
}

pub fn encode_leds(colors: &[[u8; 3]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(6 + colors.len() * 4);
    out.extend(((6 + colors.len() * 4) as u32).to_le_bytes());
    out.extend((colors.len() as u16).to_le_bytes());
    for c in colors {
        out.extend([c[0], c[1], c[2], 0]);
    }
    out
}

pub struct Client {
    s: TcpStream,
    pub protocol: u32,
}

impl Client {
    pub fn connect(addr: &str, name: &str) -> Result<Self> {
        let sock = addr.parse()?;
        let s = TcpStream::connect_timeout(&sock, Duration::from_millis(1500))?;
        s.set_read_timeout(Some(Duration::from_secs(3)))?;
        s.set_write_timeout(Some(Duration::from_secs(3)))?;
        s.set_nodelay(true)?;
        let mut c = Self { s, protocol: 0 };
        c.send(0, REQUEST_PROTOCOL_VERSION, &PROTOCOL.to_le_bytes())?;
        let server = c.recv(REQUEST_PROTOCOL_VERSION).ok().and_then(|b| b.get(..4).map(|x| u32::from_le_bytes(x.try_into().unwrap())));
        c.protocol = server.unwrap_or(0).min(PROTOCOL);
        if c.protocol < PROTOCOL {
            bail!("OpenRGB is too old (SDK protocol {}); please update to 0.8 or newer", c.protocol);
        }
        let mut n = name.as_bytes().to_vec();
        n.push(0);
        c.send(0, SET_CLIENT_NAME, &n)?;
        Ok(c)
    }

    fn send(&mut self, dev: u32, id: u32, data: &[u8]) -> Result<()> {
        let mut pkt = Vec::with_capacity(16 + data.len());
        pkt.extend(b"ORGB");
        pkt.extend(dev.to_le_bytes());
        pkt.extend(id.to_le_bytes());
        pkt.extend((data.len() as u32).to_le_bytes());
        pkt.extend(data);
        self.s.write_all(&pkt)?;
        Ok(())
    }

    fn recv(&mut self, want: u32) -> Result<Vec<u8>> {
        loop {
            let mut h = [0u8; 16];
            self.s.read_exact(&mut h)?;
            if &h[..4] != b"ORGB" {
                bail!("Bad OpenRGB header");
            }
            let id = u32::from_le_bytes(h[8..12].try_into()?);
            let len = u32::from_le_bytes(h[12..16].try_into()?) as usize;
            if len > 64 << 20 {
                bail!("OpenRGB packet too large");
            }
            let mut b = vec![0; len];
            self.s.read_exact(&mut b)?;
            if id == want {
                return Ok(b);
            } // device-list-updated notifications etc. are skipped
        }
    }

    pub fn count(&mut self) -> Result<u32> {
        self.send(0, REQUEST_CONTROLLER_COUNT, &[])?;
        let b = self.recv(REQUEST_CONTROLLER_COUNT)?;
        Ok(u32::from_le_bytes(b.get(..4).ok_or_else(|| anyhow!("short reply"))?.try_into()?))
    }

    pub fn device(&mut self, i: u32) -> Result<Device> {
        self.send(i, REQUEST_CONTROLLER_DATA, &self.protocol.to_le_bytes())?;
        parse_device(&self.recv(REQUEST_CONTROLLER_DATA)?)
    }

    pub fn devices(&mut self) -> Result<Vec<Device>> {
        (0..self.count()?).map(|i| self.device(i)).collect()
    }

    pub fn resize_zone(&mut self, dev: u32, zone: u32, size: u32) -> Result<()> {
        let mut d = (zone as i32).to_le_bytes().to_vec();
        d.extend((size as i32).to_le_bytes());
        self.send(dev, RESIZE_ZONE, &d)
    }

    pub fn set_custom_mode(&mut self, dev: u32) -> Result<()> {
        self.send(dev, SET_CUSTOM_MODE, &[])
    }

    pub fn update_mode(&mut self, dev: u32, index: i32, mode: &Mode) -> Result<()> {
        self.send(dev, UPDATE_MODE, &encode_mode(index, mode))
    }

    pub fn update_leds(&mut self, dev: u32, colors: &[[u8; 3]]) -> Result<()> {
        self.send(dev, UPDATE_LEDS, &encode_leds(colors))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(b: &mut Vec<u8>, v: &str) {
        b.extend(((v.len() + 1) as u16).to_le_bytes());
        b.extend(v.as_bytes());
        b.push(0);
    }

    #[test]
    fn parses_controller_data() {
        let mode = Mode {
            name: "Off".into(), value: 0, flags: 0, speed_min: 0, speed_max: 0, brightness_min: 0, brightness_max: 0,
            colors_min: 0, colors_max: 0, speed: 0, brightness: 0, direction: 0, color_mode: 0, colors: vec![7],
        };
        let mut b = vec![0, 0, 0, 0];
        b.extend(2i32.to_le_bytes());
        for v in ["ASUS TUF", "ASUS", "desc", "1", "sn", "loc"] {
            s(&mut b, v);
        }
        b.extend(1u16.to_le_bytes());
        b.extend(0i32.to_le_bytes());
        let enc = encode_mode(0, &mode);
        b.extend(&enc[8..]); // mode body without size/index
        b.extend(1u16.to_le_bytes());
        s(&mut b, "Aura Addressable 1");
        b.extend(1i32.to_le_bytes());
        for v in [0u32, 120, 60] {
            b.extend(v.to_le_bytes());
        }
        b.extend(0u16.to_le_bytes());
        b.extend(2u16.to_le_bytes());
        for _ in 0..2 {
            s(&mut b, "LED");
            b.extend(0u32.to_le_bytes());
        }
        b.extend(0u16.to_le_bytes());
        let d = parse_device(&b).unwrap();
        assert_eq!(d.name, "ASUS TUF");
        assert_eq!(d.modes, vec![mode]);
        assert_eq!(d.zones[0].leds_count, 60);
        assert_eq!(d.zones[0].leds_max, 120);
        assert_eq!(d.led_count, 2);
        assert!(parse_device(&b[..b.len() - 9]).is_err(), "truncated packet rejected");
    }

    #[test]
    fn encodes_leds() {
        let e = encode_leds(&[[1, 2, 3], [4, 5, 6]]);
        assert_eq!(u32::from_le_bytes(e[..4].try_into().unwrap()) as usize, e.len());
        assert_eq!(&e[4..6], &2u16.to_le_bytes());
        assert_eq!(&e[6..], &[1, 2, 3, 0, 4, 5, 6, 0]);
    }
}
