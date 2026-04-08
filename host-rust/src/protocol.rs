#[cfg(windows)]
use tokio::io::{self, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

#[cfg(windows)]
pub const MSG_VIDEO_FRAME: u8 = 0x01;
#[cfg(windows)]
pub const MSG_CONTROL_INPUT: u8 = 0x02;
#[cfg(windows)]
pub const MSG_VIDEO_KEYFRAME: u8 = 0x03;
#[cfg(windows)]
pub const MSG_VIDEO_DELTA: u8 = 0x04;
#[cfg(windows)]
pub const MSG_AUDIO_PACKET: u8 = 0x05;

#[cfg(windows)]
pub async fn write_message<W: AsyncWrite + Unpin>(
    writer: &mut W,
    msg_type: u8,
    payload: &[u8],
) -> io::Result<()> {
    writer.write_u8(msg_type).await?;
    writer.write_u32(payload.len() as u32).await?;
    writer.write_all(payload).await?;
    Ok(())
}

#[cfg(windows)]
pub async fn read_message<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<Option<(u8, Vec<u8>)>> {
    let mut msg_type = [0u8; 1];
    match reader.read_exact(&mut msg_type).await {
        Ok(_) => {}
        Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(err) => return Err(err),
    }

    let payload_len = reader.read_u32().await? as usize;
    let mut payload = vec![0u8; payload_len];
    reader.read_exact(&mut payload).await?;

    Ok(Some((msg_type[0], payload)))
}
