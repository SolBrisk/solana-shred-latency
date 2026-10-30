//! Independently implemented wire parser. Layout references:
//! https://github.com/anza-xyz/agave/blob/v4.2.2/ledger/src/shred.rs
//! https://github.com/anza-xyz/agave/blob/v4.2.2/ledger/src/shred/merkle.rs
//! https://github.com/anza-xyz/agave/blob/v4.2.2/ledger/src/shred/merkle_tree.rs
use anyhow::{bail, ensure, Result};
use sha2::{Digest, Sha256};

pub const DATA_BYTES: usize = 1203;
pub const CODE_BYTES: usize = 1228;
pub const DATA_HEADER: usize = 88;
pub const CODE_HEADER: usize = 89;
pub const MAX_INDEX: u32 = 32768;
const LEAF: &[u8] = b"\x00SOLANA_MERKLE_SHREDS_LEAF";
const NODE: &[u8] = b"\x01SOLANA_MERKLE_SHREDS_NODE";

pub fn hash(parts: &[&[u8]]) -> [u8; 32] {
    let mut h = Sha256::new();
    for part in parts {
        h.update(part);
    }
    h.finalize().into()
}
pub fn leaf(bytes: &[u8]) -> [u8; 32] {
    hash(&[LEAF, bytes])
}
pub fn join(a: &[u8], b: &[u8]) -> [u8; 32] {
    hash(&[NODE, &a[..20], &b[..20]])
}
fn u16_at(p: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(p[offset..offset + 2].try_into().unwrap())
}
fn u32_at(p: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(p[offset..offset + 4].try_into().unwrap())
}

#[derive(Clone)]
pub struct Shred<'a> {
    pub packet: &'a [u8],
    pub code: bool,
    pub resigned: bool,
    pub proof_size: usize,
    pub slot: u64,
    pub index: u32,
    pub version: u16,
    pub fec_index: u32,
    pub signature: [u8; 64],
    pub root: [u8; 32],
    pub chain: [u8; 32],
    pub proof_offset: usize,
    pub chain_offset: usize,
    pub data_count: usize,
    pub code_count: usize,
    pub position: usize,
    pub parent_offset: u16,
    pub flags: u8,
    pub size: usize,
}
impl<'a> Shred<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self> {
        ensure!(
            bytes.len() >= CODE_HEADER,
            "Packet is shorter than shred headers"
        );
        let variant = bytes[64];
        let (code, resigned) = match variant & 0xf0 {
            0x90 => (false, false),
            0xb0 => (false, true),
            0x60 => (true, false),
            0x70 => (true, true),
            _ => bail!("Unsupported shred variant; expected current chained Merkle shreds"),
        };
        let length = if code { CODE_BYTES } else { DATA_BYTES };
        ensure!(
            bytes.len() == length || bytes.len() == length + 4,
            "Unexpected payload length (only optional repair nonce is accepted)"
        );
        let packet = &bytes[..length];
        let proof_size = (variant & 15) as usize;
        ensure!(proof_size <= 8, "Proof depth exceeds GF(256) FEC geometry");
        let proof_offset = length - proof_size * 20 - usize::from(resigned) * 64;
        let chain_offset = proof_offset - 32;
        ensure!(
            chain_offset >= if code { CODE_HEADER } else { DATA_HEADER },
            "Invalid Merkle layout"
        );
        let slot = u64::from_le_bytes(packet[65..73].try_into().unwrap());
        let index = u32_at(packet, 73);
        let version = u16_at(packet, 77);
        let fec_index = u32_at(packet, 79);
        ensure!(
            index < MAX_INDEX && fec_index < MAX_INDEX,
            "Shred index outside supported slot bounds"
        );
        let (data_count, code_count, position) = if code {
            let n = u16_at(packet, 83) as usize;
            let k = u16_at(packet, 85) as usize;
            let p = u16_at(packet, 87) as usize;
            ensure!(
                n > 0
                    && k > 0
                    && n + k <= 256
                    && p < k
                    && index >= p as u32
                    && fec_index
                        .checked_add(n as u32)
                        .is_some_and(|x| x <= MAX_INDEX),
                "Invalid coding geometry"
            );
            ensure!(
                (n + k).next_power_of_two().trailing_zeros() as usize == proof_size,
                "Coding geometry does not match proof depth"
            );
            (n, k, p)
        } else {
            ensure!(
                index >= fec_index && index - fec_index < 256,
                "Invalid data index"
            );
            (0, 0, (index - fec_index) as usize)
        };
        let (parent_offset, flags, size) = if code {
            (0, 0, 0)
        } else {
            let parent = u16_at(packet, 83);
            let flags = packet[85];
            let size = u16_at(packet, 86) as usize;
            ensure!(
                (parent as u64) <= slot && (parent != 0 || slot == 0),
                "Invalid parent offset"
            );
            ensure!(
                size >= DATA_HEADER && size <= chain_offset,
                "Invalid data length"
            );
            ensure!(
                flags & 0x80 == 0 || flags & 0x40 != 0,
                "Last-in-slot must imply data-complete"
            );
            (parent, flags, size)
        };
        let signature = packet[..64].try_into().unwrap();
        let chain = packet[chain_offset..proof_offset].try_into().unwrap();
        let mut root = leaf(&packet[64..proof_offset]);
        let mut node = if code {
            data_count + position
        } else {
            position
        };
        ensure!(
            node < (1usize << proof_size),
            "Shred position exceeds proof tree"
        );
        for sibling in packet[proof_offset..proof_offset + proof_size * 20].chunks_exact(20) {
            root = if node & 1 == 0 {
                join(&root, sibling)
            } else {
                join(sibling, &root)
            };
            node >>= 1;
        }
        Ok(Self {
            packet,
            code,
            resigned,
            proof_size,
            slot,
            index,
            version,
            fec_index,
            signature,
            root,
            chain,
            proof_offset,
            chain_offset,
            data_count,
            code_count,
            position,
            parent_offset,
            flags,
            size,
        })
    }
    pub fn data(&self) -> &'a [u8] {
        if self.code {
            &[]
        } else {
            &self.packet[DATA_HEADER..self.size]
        }
    }
    pub fn erasure(&self) -> &'a [u8] {
        &self.packet[if self.code { CODE_HEADER } else { 64 }..self.chain_offset]
    }
    /// Match logical data, not a relay's Merkle-tail representation. Include
    /// leader signature and application-bearing headers/payload, so distinct
    /// forks or conflicting data cannot be counted as one event.
    pub fn identity(&self) -> [u8; 32] {
        let end = if self.code {
            self.chain_offset
        } else {
            self.size
        };
        hash(&[
            &self.signature,
            &self.slot.to_le_bytes(),
            &self.index.to_le_bytes(),
            &[u8::from(self.code)],
            &self.version.to_le_bytes(),
            &self.packet[65..end],
        ])
    }
}

pub fn verification(packet: &[u8]) -> Option<&str> {
    let text = std::str::from_utf8(packet).ok()?;
    let fields = text.split_whitespace().collect::<Vec<_>>();
    if fields.len() == 3
        && fields[0] == "SHREDHARBOR_VERIFY"
        && fields[2].len() == 48
        && fields[2].bytes().all(|b| b.is_ascii_hexdigit())
    {
        Some(fields[2])
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    pub fn data_packet(index: u32, flags: u8, data: &[u8]) -> Vec<u8> {
        let mut p = vec![0; DATA_BYTES];
        p[64] = 0x96;
        p[65..73].copy_from_slice(&10u64.to_le_bytes());
        p[73..77].copy_from_slice(&index.to_le_bytes());
        p[77..79].copy_from_slice(&1u16.to_le_bytes());
        p[83..85].copy_from_slice(&1u16.to_le_bytes());
        p[85] = flags;
        p[86..88].copy_from_slice(&((DATA_HEADER + data.len()) as u16).to_le_bytes());
        p[88..88 + data.len()].copy_from_slice(data);
        p
    }
    #[test]
    fn validates_headers_and_extracts_data() {
        let p = data_packet(0, 0xc0, b"abc");
        let s = Shred::parse(&p).unwrap();
        assert_eq!(s.data(), b"abc");
        assert_eq!(s.slot, 10);
        assert_eq!(s.flags, 0xc0);
    }
    #[test]
    fn rejects_malformed_lengths_and_offsets() {
        let mut p = data_packet(0, 0x40, b"abc");
        p[86..88].copy_from_slice(&65535u16.to_le_bytes());
        assert!(Shred::parse(&p).is_err());
        assert!(Shred::parse(&[0; 70]).is_err());
    }
    #[test]
    fn control_packets_are_not_shreds() {
        let p = b"SHREDHARBOR_VERIFY example 0123456789abcdef0123456789abcdef0123456789abcdef";
        assert!(verification(p).is_some());
        assert!(Shred::parse(p).is_err());
    }
}
