use flate2::read::ZlibDecoder;
use std::{
    collections::HashMap,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

type Result<T> = std::result::Result<T, String>;
const INVALID: &str = "MDict 文件格式不受支持或文件已损坏";
const LIMIT: usize = 64 * 1024 * 1024;

fn bytes(reader: &mut impl Read, length: usize) -> Result<Vec<u8>> {
    if length > LIMIT {
        return Err(INVALID.into());
    }
    let mut data = vec![0; length];
    reader.read_exact(&mut data).map_err(|_| INVALID)?;
    Ok(data)
}

fn number(reader: &mut impl Read, width: usize) -> Result<u64> {
    Ok(bytes(reader, width)?
        .into_iter()
        .fold(0, |n, byte| n << 8 | u64::from(byte)))
}

fn expand(data: &[u8], size: usize) -> Result<Vec<u8>> {
    if size > LIMIT {
        return Err(INVALID.into());
    }
    let payload = data.get(8..).ok_or(INVALID)?;
    let out = match data.first() {
        Some(0) => payload.to_vec(),
        Some(2) => {
            let mut out = Vec::new();
            ZlibDecoder::new(payload)
                .take(size as u64 + 1)
                .read_to_end(&mut out)
                .map_err(|_| INVALID)?;
            out
        }
        _ => return Err(INVALID.into()),
    };
    if out.len() != size {
        return Err(INVALID.into());
    }
    Ok(out)
}

fn decode(bytes: &[u8], width: usize) -> Result<String> {
    if width == 1 {
        return String::from_utf8(bytes.to_vec()).map_err(|_| INVALID.into());
    }
    if bytes.len() % 2 != 0 {
        return Err(INVALID.into());
    }
    String::from_utf16(
        &bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect::<Vec<_>>(),
    )
    .map_err(|_| INVALID.into())
}

#[derive(Debug)]
struct Block {
    file: u64,
    start: u64,
    packed: usize,
    size: usize,
}

#[derive(Debug)]
pub struct Archive {
    path: PathBuf,
    keys: HashMap<String, (u64, u64)>,
    blocks: Vec<Block>,
}

pub fn resource_key(value: &str) -> String {
    format!("\\{}", value.replace('/', "\\").trim_start_matches('\\')).to_lowercase()
}

impl Archive {
    pub fn open(path: &Path, resource: bool) -> Result<Self> {
        let mut file = File::open(path).map_err(|e| e.to_string())?;
        let header_len = number(&mut file, 4)? as usize;
        let header = decode(&bytes(&mut file, header_len)?, 2)?;
        if !header.contains("GeneratedByEngineVersion=\"2.")
            || !(resource || header.contains("Encoding=\"UTF-8\""))
            || header.contains("Encrypted=\"1\"")
            || header.contains("Encrypted=\"3\"")
        {
            return Err(INVALID.into());
        }
        number(&mut file, 4)?;
        let block_count = number(&mut file, 8)?;
        let entry_count = number(&mut file, 8)?;
        let info_size = number(&mut file, 8)? as usize;
        let info_packed = number(&mut file, 8)? as usize;
        let key_size = number(&mut file, 8)?;
        number(&mut file, 4)?;
        if block_count > 1_000_000 || entry_count > 2_000_000 {
            return Err(INVALID.into());
        }
        let mut info = bytes(&mut file, info_packed)?;
        if header.contains("Encrypted=\"2\"") {
            decode_index(&mut info)?;
        }
        let info = expand(&info, info_size).map_err(|error| format!("索引头：{error}"))?;
        let mut info = std::io::Cursor::new(info.as_slice());
        let key_start = file.stream_position().map_err(|_| INVALID)?;
        let width = if resource { 2 } else { 1 };
        let mut entries = Vec::new();
        for _ in 0..block_count {
            let count = number(&mut info, 8)?;
            for _ in 0..2 {
                let len = number(&mut info, 2)? as usize;
                bytes(&mut info, (len + 1) * width)?;
            }
            let packed = number(&mut info, 8)? as usize;
            let size = number(&mut info, 8)? as usize;
            let data = expand(&bytes(&mut file, packed)?, size)
                .map_err(|error| format!("词条索引：{error}"))?;
            let mut i = 0;
            for _ in 0..count {
                let offset = u64::from_be_bytes(
                    data.get(i..i + 8)
                        .ok_or(INVALID)?
                        .try_into()
                        .map_err(|_| INVALID)?,
                );
                i += 8;
                let len = data[i..]
                    .chunks_exact(width)
                    .position(|chunk| chunk.iter().all(|&b| b == 0))
                    .ok_or(INVALID)?
                    * width;
                let key = decode(&data[i..i + len], width)?;
                entries.push((
                    if resource {
                        resource_key(&key)
                    } else {
                        key.to_lowercase()
                    },
                    offset,
                ));
                i += len + width;
            }
        }
        if entries.len() as u64 != entry_count
            || file.stream_position().map_err(|_| INVALID)? - key_start != key_size
        {
            return Err(INVALID.into());
        }
        let count = number(&mut file, 8)?;
        if count > 1_000_000
            || number(&mut file, 8)? != entry_count
            || number(&mut file, 8)? != count * 16
        {
            return Err(INVALID.into());
        }
        let packed_total = number(&mut file, 8)?;
        let mut position = file.stream_position().map_err(|_| INVALID)? + count * 16;
        let end = position.checked_add(packed_total).ok_or(INVALID)?;
        if end > file.metadata().map_err(|_| INVALID)?.len() {
            return Err(INVALID.into());
        }
        let mut offset = 0u64;
        let mut blocks = Vec::new();
        for _ in 0..count {
            let packed = number(&mut file, 8)? as usize;
            let size = number(&mut file, 8)? as usize;
            if packed > LIMIT || size > LIMIT || size == 0 {
                return Err(INVALID.into());
            }
            blocks.push(Block {
                file: position,
                start: offset,
                packed,
                size,
            });
            position = position.checked_add(packed as u64).ok_or(INVALID)?;
            offset += size as u64;
        }
        if position != end {
            return Err(INVALID.into());
        }
        let mut keys = HashMap::new();
        let mut next = offset;
        for (key, start) in entries.into_iter().rev() {
            if start > next {
                return Err(INVALID.into());
            }
            if start != next {
                keys.insert(key, (start, next));
            }
            next = start;
        }
        Ok(Self {
            path: path.to_owned(),
            keys,
            blocks,
        })
    }

    pub fn contains(&self, key: &str) -> bool {
        self.keys.contains_key(key)
    }

    pub fn read(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let Some(&(mut start, end)) = self.keys.get(key) else {
            return Ok(None);
        };
        if end - start > LIMIT as u64 {
            return Err(INVALID.into());
        }
        let mut file = File::open(&self.path).map_err(|_| "词典文件不可用")?;
        let mut result = Vec::with_capacity((end - start) as usize);
        while start < end {
            let index = self.blocks.partition_point(|block| block.start <= start);
            let block = self
                .blocks
                .get(index.checked_sub(1).ok_or(INVALID)?)
                .ok_or(INVALID)?;
            file.seek(SeekFrom::Start(block.file))
                .map_err(|_| INVALID)?;
            let data = expand(&bytes(&mut file, block.packed)?, block.size)?;
            let stop = end.min(block.start + block.size as u64);
            result.extend_from_slice(
                &data[(start - block.start) as usize..(stop - block.start) as usize],
            );
            start = stop;
        }
        Ok(Some(result))
    }
}

// MDict v2 obfuscates key-block metadata using a RIPEMD-128 digest of the block checksum.
fn decode_index(data: &mut [u8]) -> Result<()> {
    let checksum: [u8; 4] = data
        .get(4..8)
        .ok_or(INVALID)?
        .try_into()
        .map_err(|_| INVALID)?;
    let key = index_digest(checksum);
    let mut previous = 0x36;
    for (index, byte) in data[8..].iter_mut().enumerate() {
        let original = *byte;
        *byte = original.rotate_left(4) ^ previous ^ index as u8 ^ key[index % 16];
        previous = original;
    }
    Ok(())
}

fn index_digest(checksum: [u8; 4]) -> [u8; 16] {
    const LEFT: [[usize; 16]; 4] = [
        [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
        [7, 4, 13, 1, 10, 6, 15, 3, 12, 0, 9, 5, 2, 14, 11, 8],
        [3, 10, 14, 4, 9, 15, 8, 1, 2, 7, 0, 6, 13, 11, 5, 12],
        [1, 9, 11, 10, 0, 8, 12, 4, 13, 3, 7, 15, 14, 5, 6, 2],
    ];
    const RIGHT: [[usize; 16]; 4] = [
        [5, 14, 7, 0, 9, 2, 11, 4, 13, 6, 15, 8, 1, 10, 3, 12],
        [6, 11, 3, 7, 0, 13, 5, 10, 14, 15, 8, 12, 4, 9, 1, 2],
        [15, 5, 1, 3, 7, 14, 6, 9, 11, 8, 12, 2, 10, 0, 4, 13],
        [8, 6, 4, 1, 3, 11, 15, 0, 5, 12, 2, 13, 9, 7, 10, 14],
    ];
    const LS: [[u32; 16]; 4] = [
        [11, 14, 15, 12, 5, 8, 7, 9, 11, 13, 14, 15, 6, 7, 9, 8],
        [7, 6, 8, 13, 11, 9, 7, 15, 7, 12, 15, 9, 11, 7, 13, 12],
        [11, 13, 6, 7, 14, 9, 13, 15, 14, 8, 13, 6, 5, 12, 7, 5],
        [11, 12, 14, 15, 14, 15, 9, 8, 9, 14, 5, 6, 8, 6, 5, 12],
    ];
    const RS: [[u32; 16]; 4] = [
        [8, 9, 9, 11, 13, 15, 15, 5, 7, 7, 8, 11, 14, 14, 12, 6],
        [9, 13, 15, 7, 12, 8, 9, 11, 7, 7, 12, 7, 6, 15, 13, 11],
        [9, 7, 15, 11, 8, 6, 6, 14, 12, 13, 5, 14, 13, 13, 7, 5],
        [15, 5, 8, 11, 14, 14, 6, 14, 6, 9, 12, 9, 12, 5, 15, 8],
    ];
    let mut words = [0u32; 16];
    words[0] = u32::from_le_bytes(checksum);
    words[1] = 0x3695;
    words[2] = 0x80;
    words[14] = 64;
    let initial = [0x67452301u32, 0xefcdab89, 0x98badcfe, 0x10325476];
    let lane =
        |indices: [[usize; 16]; 4], shifts: [[u32; 16]; 4], constants: [u32; 4], reverse: bool| {
            let [mut a, mut b, mut c, mut d] = initial;
            for round in 0..4 {
                for i in 0..16 {
                    let mixed = match if reverse { 3 - round } else { round } {
                        0 => b ^ c ^ d,
                        1 => (b & c) | (!b & d),
                        2 => (b | !c) ^ d,
                        _ => (b & d) | (c & !d),
                    };
                    let next = a
                        .wrapping_add(mixed)
                        .wrapping_add(words[indices[round][i]])
                        .wrapping_add(constants[round])
                        .rotate_left(shifts[round][i]);
                    [a, b, c, d] = [d, next, b, c];
                }
            }
            [a, b, c, d]
        };
    let [a, b, c, d] = lane(LEFT, LS, [0, 0x5a827999, 0x6ed9eba1, 0x8f1bbcdc], false);
    let [aa, bb, cc, dd] = lane(RIGHT, RS, [0x50a28be6, 0x5c4dd124, 0x6d703ef3, 0], true);
    let mut key = [0; 16];
    for (i, value) in [
        initial[1].wrapping_add(c).wrapping_add(dd),
        initial[2].wrapping_add(d).wrapping_add(aa),
        initial[3].wrapping_add(a).wrapping_add(bb),
        initial[0].wrapping_add(b).wrapping_add(cc),
    ]
    .into_iter()
    .enumerate()
    {
        key[i * 4..i * 4 + 4].copy_from_slice(&value.to_le_bytes());
    }
    key
}

#[cfg(test)]
mod tests {
    #[test]
    fn mdict_v2_key_checksum_digest() {
        assert_eq!(
            super::index_digest([0x29, 0xf8, 0xe6, 0xa2]),
            [
                0x93, 0x2e, 0xe0, 0x76, 0xaf, 0x6f, 0x34, 0x9a, 0x51, 0x94, 0x77, 0xea, 0x72, 0x71,
                0xe1, 0x20
            ]
        );
    }
}
