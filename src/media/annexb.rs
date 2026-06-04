//! Annex-B H.264 helpers: split a byte stream into NAL units and join NAL
//! units back into an Annex-B access unit.

/// Split an Annex-B byte stream into raw NAL units (start codes removed).
#[must_use]
pub fn split_nals(data: &[u8]) -> Vec<Vec<u8>> {
    let mut nals = Vec::new();
    let starts = start_code_positions(data);
    for window in 0..starts.len() {
        let (pos, sc_len) = starts[window];
        let nal_start = pos + sc_len;
        let nal_end = if window + 1 < starts.len() {
            starts[window + 1].0
        } else {
            data.len()
        };
        if nal_start < nal_end {
            nals.push(data[nal_start..nal_end].to_vec());
        }
    }
    nals
}

/// Join NAL units into an Annex-B access unit, prefixing each with a 4-byte
/// start code.
#[must_use]
pub fn join_nals(nals: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for nal in nals {
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(nal);
    }
    out
}

fn start_code_positions(data: &[u8]) -> Vec<(usize, usize)> {
    let mut positions = Vec::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 {
            if i + 4 <= data.len() && data[i + 2] == 0 && data[i + 3] == 1 {
                positions.push((i, 4));
                i += 4;
                continue;
            }
            if data[i + 2] == 1 {
                positions.push((i, 3));
                i += 3;
                continue;
            }
        }
        i += 1;
    }
    positions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let nals = vec![vec![0x67, 0x42, 0x00], vec![0x68, 0xce], vec![0x65, 0x88, 0x77]];
        let annexb = join_nals(&nals);
        assert_eq!(split_nals(&annexb), nals);
    }

    #[test]
    fn three_byte_start_codes() {
        let data = [0, 0, 1, 0x67, 0xaa, 0, 0, 1, 0x65, 0xbb];
        assert_eq!(split_nals(&data), vec![vec![0x67, 0xaa], vec![0x65, 0xbb]]);
    }
}
