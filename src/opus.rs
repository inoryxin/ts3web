//! 解析 Opus 的 TOC 字节，算出这一帧音频的时长（毫秒）。
//!
//! 为什么需要它：TS3 收到的是一个个 Opus 包，浏览器侧的 `AudioDecoder` 需要每个
//! 包带上正确的 `duration` 才能正确排期播放。Opus 包的时长由 TOC 字节决定：
//! 高 5 位是 config（决定单帧时长），低 2 位是帧数编码。
//! 参考 RFC 6716 §3.1。

/// 一个 Opus 包的总时长（毫秒）。解析失败时返回 None，调用方按 20ms 兜底。
pub fn packet_duration_ms(payload: &[u8]) -> Option<u32> {
    let toc = *payload.first()?;

    // config: 0..31，决定"单帧"时长
    let config = toc >> 3;
    let frame_ms = match config {
        // 0..11：SILK / Hybrid，NB/MB/WB，10/20/40/60ms
        0..=11 => match config % 4 {
            0 => 10,
            1 => 20,
            2 => 40,
            _ => 60,
        },
        // 12..15：Hybrid SWB/FB，10 或 20ms
        12..=15 => {
            if config % 2 == 0 {
                10
            } else {
                20
            }
        }
        // 16..31：CELT，2.5/5/10/20ms 循环
        _ => match (config - 16) % 4 {
            0 => 3, // 2.5ms，向上取整到 3ms 避免 0 时长
            1 => 5,
            2 => 10,
            _ => 20,
        },
    };

    // 低 2 位 c：帧数编码
    let frames = match toc & 0x03 {
        0 => 1u32,
        1 | 2 => 2,
        _ => {
            // c == 3：帧数写在第二个字节的低 6 位
            match payload.get(1) {
                Some(b) => (*b & 0x3F) as u32,
                None => return None,
            }
        }
    };

    let total = frame_ms * frames.max(1);
    if total == 0 {
        None
    } else {
        Some(total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_20ms_single_frame() {
        // config = 1 (SILK NB 20ms), c = 0 -> 1 帧
        let toc = (1u8 << 3) | 0;
        assert_eq!(packet_duration_ms(&[toc, 0, 0]), Some(20));
    }

    #[test]
    fn celt_20ms_two_frames() {
        // config = 19 (CELT 20ms), c = 1 -> 2 帧
        let toc = (19u8 << 3) | 1;
        assert_eq!(packet_duration_ms(&[toc, 0]), Some(40));
    }

    #[test]
    fn arbitrary_frame_count() {
        // config = 19, c = 3 -> 帧数在第二字节低 6 位
        let toc = (19u8 << 3) | 3;
        assert_eq!(packet_duration_ms(&[toc, 3]), Some(60));
    }

    #[test]
    fn empty_is_none() {
        assert_eq!(packet_duration_ms(&[]), None);
    }
}
