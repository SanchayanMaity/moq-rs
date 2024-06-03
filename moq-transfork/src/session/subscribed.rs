use futures::stream::FuturesUnordered;
use futures::StreamExt;

use crate::serve::ServeError;
use crate::{message, serve};

use super::{Control, SessionError, Writer};

#[derive(Clone)]
pub struct Subscribed {
	session: web_transport::Session,
	subscribe: message::Subscribe,
	update: Option<message::SubscribeUpdate>,
	track: serve::TrackReader,
}

impl Subscribed {
	pub(super) fn new(
		session: web_transport::Session,
		subscribe: message::Subscribe,
		track: serve::TrackReader,
	) -> Self {
		Self {
			session,
			subscribe,
			update: None,
			track,
		}
	}

	pub async fn run(mut self, mut control: Control) -> Result<(), SessionError> {
		let mut tasks = FuturesUnordered::new();
		let mut fin = false;

		loop {
			tokio::select! {
				res = self.track.next_group(), if !fin => {
					let group = match res? {
						Some(group) => group,
						None => {
							fin = true;
							continue;
						},
					};

					let sequence = group.sequence;
					let this = self.clone();

					tasks.push(async move {
						let err = Self::run_group(this, group).await;
						(sequence, err)
					});
				},
				Some(res) = control.reader.decode_maybe::<message::SubscribeUpdate>() => {
					let update = res?;
					self.recv_update(update)?;
				},
				res = tasks.next(), if !tasks.is_empty() => {
					let (sequence, err) = res.unwrap();

					if let Err(_) = err {
						let msg = message::GroupDrop {
							sequence,
							count: 0,
							code: 1, // TODO err.code()
						};
						control.writer.encode(&msg).await?;
					}
				},
			}
		}
	}

	pub async fn run_group(mut self, mut group: serve::GroupReader) -> Result<(), SessionError> {
		let stream = self.session.open_uni().await?;

		let mut writer = Writer::new(stream);

		let msg = message::Group {
			subscribe: self.subscribe.id,
			sequence: group.sequence,
			expires: group.expires,
		};

		writer.encode(&msg).await?;

		// TODO abort if the subscription is closed

		while let Some(chunk) = group.read().await? {
			writer.write(&chunk).await?;
		}

		// TODO block until all bytes have been acknowledged so we can still reset
		// writer.finish().await?;

		Ok(())
	}

	fn recv_update(&mut self, update: message::SubscribeUpdate) -> Result<(), ServeError> {
		todo!("SubscribeUpdate");
		self.update = Some(update);
		Ok(())
	}
}
