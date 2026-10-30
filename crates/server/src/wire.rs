//! y-sync 线协议的最小实现（lib0 varuint 帧）。
//!
//! 布局与 y-sync 0.4 / y-websocket 一致：
//! - `tag: varuint`
//! - drop-in `Sync`：`subtype: varuint` + `buf`（`len: varuint` + 载荷）
//! - `Awareness`：`buf`（载荷不解析，原样转发）
//!
//! 只有服务端需要的部分：同步三步（step1/step2/update）与 awareness 转发。
//! 状态向量与 update 的载荷仍由 `yrs` 编解码（v1）。

pub const MSG_SYNC: u64 = 0;
pub const MSG_AWARENESS: u64 = 1;
pub const MSG_AWARENESS_QUERY: u64 = 3;

pub const SYNC_STEP1: u64 = 0;
pub const SYNC_STEP2: u64 = 1;
pub const SYNC_UPDATE: u64 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireMessage {
    /// 对端状态向量（v1 编码）。
    SyncStep1(Vec<u8>),
    /// 差量 update（v1 编码）。
    SyncStep2(Vec<u8>),
    /// 增量 update（v1 编码）。
    Update(Vec<u8>),
    /// awareness 载荷（不透明，只转发）。
    Awareness(Vec<u8>),
    AwarenessQuery,
    Other,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WireError {
    #[error("帧被截断")]
    Truncated,
    #[error("varint 溢出")]
    Overflow,
    #[error("未知的同步子类型")]
    UnknownSyncSubtype,
}

/// lib0 varuint（小端、7 位分组、最高位续位）。
pub fn write_var(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

pub fn read_var(input: &[u8], pos: &mut usize) -> Result<u64, WireError> {
    let mut value = 0u64;
    let mut shift = 0u32;
    loop {
        let byte = *input.get(*pos).ok_or(WireError::Truncated)?;
        *pos += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
        if shift >= 64 {
            return Err(WireError::Overflow);
        }
    }
}

fn write_buf(out: &mut Vec<u8>, bytes: &[u8]) {
    write_var(out, u64::try_from(bytes.len()).unwrap_or(u64::MAX));
    out.extend_from_slice(bytes);
}

fn read_buf<'a>(input: &'a [u8], pos: &mut usize) -> Result<&'a [u8], WireError> {
    let len = usize::try_from(read_var(input, pos)?).map_err(|_| WireError::Overflow)?;
    let end = pos.checked_add(len).ok_or(WireError::Overflow)?;
    let slice = input.get(*pos..end).ok_or(WireError::Truncated)?;
    *pos = end;
    Ok(slice)
}

pub fn parse(frame: &[u8]) -> Result<WireMessage, WireError> {
    let mut pos = 0;
    let tag = read_var(frame, &mut pos)?;
    match tag {
        MSG_SYNC => {
            let subtype = read_var(frame, &mut pos)?;
            let payload = read_buf(frame, &mut pos)?.to_vec();
            match subtype {
                SYNC_STEP1 => Ok(WireMessage::SyncStep1(payload)),
                SYNC_STEP2 => Ok(WireMessage::SyncStep2(payload)),
                SYNC_UPDATE => Ok(WireMessage::Update(payload)),
                _ => Err(WireError::UnknownSyncSubtype),
            }
        }
        MSG_AWARENESS => Ok(WireMessage::Awareness(read_buf(frame, &mut pos)?.to_vec())),
        MSG_AWARENESS_QUERY => Ok(WireMessage::AwarenessQuery),
        _ => Ok(WireMessage::Other),
    }
}

pub fn sync_step1(state_vector: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(state_vector.len() + 8);
    write_var(&mut out, MSG_SYNC);
    write_var(&mut out, SYNC_STEP1);
    write_buf(&mut out, state_vector);
    out
}

pub fn sync_step2(update: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(update.len() + 8);
    write_var(&mut out, MSG_SYNC);
    write_var(&mut out, SYNC_STEP2);
    write_buf(&mut out, update);
    out
}

pub fn update(update: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(update.len() + 8);
    write_var(&mut out, MSG_SYNC);
    write_var(&mut out, SYNC_UPDATE);
    write_buf(&mut out, update);
    out
}

pub fn awareness(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 8);
    write_var(&mut out, MSG_AWARENESS);
    write_buf(&mut out, payload);
    out
}

/// 廉价判断一帧是否会触发写入（SyncStep2 / Update）；用于写前撤销复核。
pub fn is_write_frame(frame: &[u8]) -> bool {
    let mut pos = 0;
    if read_var(frame, &mut pos) != Ok(MSG_SYNC) {
        return false;
    }
    matches!(read_var(frame, &mut pos), Ok(SYNC_STEP2) | Ok(SYNC_UPDATE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varint_roundtrip() {
        for value in [0u64, 1, 127, 128, 300, 16_383, 16_384, u32::MAX as u64] {
            let mut buf = Vec::new();
            write_var(&mut buf, value);
            let mut pos = 0;
            assert_eq!(read_var(&buf, &mut pos), Ok(value));
            assert_eq!(pos, buf.len());
        }
    }

    #[test]
    fn truncated_varint_is_rejected() {
        let mut pos = 0;
        assert_eq!(read_var(&[0x80], &mut pos), Err(WireError::Truncated));
        // 11 个续位字节：移位超过 u64 宽度，必须报 Overflow 而不是静默截断。
        let mut pos = 0;
        assert_eq!(read_var(&[0xff; 11], &mut pos), Err(WireError::Overflow));
    }

    #[test]
    fn sync_frames_roundtrip() {
        let payload = vec![1u8, 2, 3, 4, 5];

        let step1 = sync_step1(&payload);
        assert_eq!(parse(&step1), Ok(WireMessage::SyncStep1(payload.clone())));

        let step2 = sync_step2(&payload);
        assert_eq!(parse(&step2), Ok(WireMessage::SyncStep2(payload.clone())));

        let upd = update(&payload);
        assert_eq!(parse(&upd), Ok(WireMessage::Update(payload.clone())));

        let aware = awareness(&payload);
        assert_eq!(parse(&aware), Ok(WireMessage::Awareness(payload)));
    }

    #[test]
    fn wire_layout_matches_y_sync() {
        // tag=0(Sync), subtype=2(Update), len=3, payload=[9,8,7]
        let frame = update(&[9, 8, 7]);
        assert_eq!(frame, vec![0, 2, 3, 9, 8, 7]);
    }

    #[test]
    fn unknown_tags_are_ignored_not_fatal() {
        assert_eq!(parse(&[9, 0]), Ok(WireMessage::Other));
        assert_eq!(parse(&[3]), Ok(WireMessage::AwarenessQuery));
    }
}
