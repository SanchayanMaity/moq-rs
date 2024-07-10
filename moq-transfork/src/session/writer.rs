use std::fmt;

use super::{Close, SessionError};
use crate::{coding::*, message};

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

	pub async fn open(session: &mut web_transport::Session, typ: message::StreamUni) -> Result<Self, SessionError> {
		let send = session.open_uni().await?;

		let mut writer = Self::new(send);
		writer.encode_silent(&typ).await?;

		Ok(writer)
	}

	pub async fn encode<T: Encode + fmt::Debug>(&mut self, msg: &T) -> Result<(), SessionError> {
		tracing::debug!(?msg, "encode");
		self.encode_silent(msg).await
	}

	// A separate function just to avoid an extra log line
	pub async fn encode_silent<T: Encode + fmt::Debug>(&mut self, msg: &T) -> Result<(), SessionError> {
		self.buffer.clear();

		msg.encode(&mut self.buffer)?;

		while !self.buffer.is_empty() {
			self.stream.write_buf(&mut self.buffer).await?;
		}

		Ok(())
	}

	pub async fn write(&mut self, buf: &[u8]) -> Result<(), SessionError> {
		self.stream.write(buf).await?; // convert the error type
		Ok(())
	}
}

impl Close for Writer {
	fn close(&mut self, code: u32) {
		self.stream.reset(code);
	}
}