use std::time;

use super::{Error, Result, Root};

pub struct Reader {
	track: moq_transfork::TrackReader,
}

impl Reader {
	pub fn new(track: moq_transfork::TrackReader) -> Self {
		Self { track }
	}

	pub async fn subscribe(broadcast: moq_transfork::BroadcastReader) -> Result<Self> {
		let track = moq_transfork::Track::build("catalog.json", 0)
			.group_order(moq_transfork::GroupOrder::Descending)
			.group_expires(time::Duration::ZERO)
			.into();
		let track = broadcast.subscribe(track).await?;
		Ok(Self::new(track))
	}

	pub async fn read(&mut self) -> Result<Root> {
		let mut group = self.track.next_group().await?.ok_or(Error::Empty)?;
		let frame = group.read_frame().await?.ok_or(Error::Empty)?;
		Root::from_slice(&frame)
	}

	// TODO support updates
}
