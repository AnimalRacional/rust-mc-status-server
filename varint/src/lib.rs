use std::io::{Error, Read};

pub fn decode_stream<T: Read>(stream: &mut T) -> Result<i32, Error> {
    let mut shift: u8 = 0;
    let mut result: i32 = 0;
    let mut buf: [u8; 1] = [0];
    loop {
        let i: Result<usize, Error> = stream.read(&mut buf);
        let i = match i {
            Ok(_) => buf[0] as i32,
            Err(err) => {
                return Err(err);
            }
        };
        result = result | ((i & 0x7f) << shift);
        shift += 7;
        if i & 0x80 == 0 {
            break;
        }
    }
    Ok(result)
}

pub fn encode(n: i32) -> Vec<u8> {
    let mut res = Vec::<u8>::new();
    let mut cur = n;
    loop {
        let b = (cur & 0x7f) as u8;
        cur = cur >> 7;
        if cur == 0 {
            res.push(b);
            break;
        }
        res.push(b | 0x80);
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_payload(vect: Vec<u8>, trg: i32) {
        let mut bytes = vect.as_slice();
        let res = decode_stream(&mut bytes).expect("error decoding stream");
        assert_eq!(res, trg);
        let res = encode(trg);
        assert_eq!(res, vect);
    }

    #[test]
    fn test_127() {
        check_payload(vec![0x7f], 127);
    }

    #[test]
    fn test_128() {
        check_payload(vec![0x80, 0x01], 128)
    }

    #[test]
    fn test_4bytes() {
        check_payload(0xd88bad01u32.to_be_bytes().to_vec(), 2835928);
    }

    #[test]
    fn check_equal_4bytes() {
        for i in 0..(0xffffu32) {
            let i = i as i32;
            let encoded = encode(i);
            let mut encoded = encoded.as_slice();
            assert_eq!(decode_stream(&mut encoded).unwrap(), i);
        }
    }
}
