use std::fmt;

use crate::coding::*;
use crate::util::Close;
use crate::MoqError;

pub struct Writer {
	stream: web_transport::SendStream,
	buffer: bytes::BytesMut,
}

impl Writer {
	pub fn new(stream: web_transport::SendStream) -> Self {
		Self {
			stream,
			buffer: Default::default(),
		}
	}

	pub async fn encode<T: Encode + fmt::Debug>(&mut self, msg: &T) -> Result<(), MoqError> {
		tracing::debug!(?msg, "encode");
		self.encode_silent(msg).await
	}

	// A separate function just to avoid an extra log line
	pub async fn encode_silent<T: Encode + fmt::Debug>(&mut self, msg: &T) -> Result<(), MoqError> {
		self.buffer.clear();

		msg.encode(&mut self.buffer)?;

		while !self.buffer.is_empty() {
			self.stream.write_buf(&mut self.buffer).await?;
		}

		Ok(())
	}

	pub async fn write(&mut self, buf: &[u8]) -> Result<(), MoqError> {
		self.stream.write(buf).await?; // convert the error type
		Ok(())
	}
}

impl Close for Writer {
	fn close(&mut self, err: MoqError) {
		self.stream.reset(err.to_code());
	}
}
