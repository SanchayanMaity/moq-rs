use std::{
	collections::{hash_map, HashMap},
	sync::{atomic, Arc},
};

use tracing::Instrument;

use crate::{
	message,
	model::{Broadcast, BroadcastReader, Produce, Router, Track, TrackReader, TrackWriter},
	runtime::{self, Lock, Queue},
	util::OrClose,
	BroadcastWriter, Error, RouterWriter,
};

use super::{Reader, Session, Stream};

#[derive(Clone)]
pub struct Subscriber {
	session: Session,
	announced: Queue<BroadcastReader>,

	broadcasts: Lock<HashMap<String, BroadcastReader>>,
	tracks: Lock<HashMap<u64, TrackWriter>>,
	next_id: Arc<atomic::AtomicU64>,
}

impl Subscriber {
	pub(super) fn new(session: Session) -> Self {
		Self {
			session,
			announced: Default::default(),

			broadcasts: Default::default(),
			tracks: Default::default(),
			next_id: Default::default(),
		}
	}

	// TODO make a handle so there can be multiple subscribers
	pub async fn announced(&mut self) -> Option<BroadcastReader> {
		self.announced.pop().await
	}

	// TODO come up with a better name
	/// Subscribe to tracks from a given broadcast.
	///
	/// This is a helper method to avoid waiting for an (optional) [Self::announced] or cloning the [Broadcast] for each [Self::subscribe].
	pub fn namespace<T: Into<Broadcast>>(&self, broadcast: T) -> Result<BroadcastReader, Error> {
		let broadcast = broadcast.into();
		let (mut writer, reader) = broadcast.clone().produce();

		match self.broadcasts.lock().entry(broadcast.name.clone()) {
			hash_map::Entry::Occupied(entry) => return Ok(entry.get().clone()),
			hash_map::Entry::Vacant(entry) => entry.insert(reader.clone()),
		};

		let router = Router::produce();
		writer.route_tracks(router.1)?;

		let announce = Announce {
			broadcast: writer,
			router: router.0,
			broadcasts: self.broadcasts.clone(),
		};

		let span = tracing::info_span!("announce", broadcast = broadcast.name);
		runtime::spawn(self.clone().run_announce(announce).instrument(span));

		Ok(reader)
	}

	async fn run_announce(self, mut announce: Announce) {
		while let Some(request) = announce.router.requested().await {
			let mut this = self.clone();
			let broadcast = announce.broadcast.info.as_ref().clone();

			runtime::spawn(async move {
				match this.subscribe(broadcast, request.info.clone()).await {
					Ok(track) => request.serve(track),
					Err(err) => request.close(err),
				};
			});
		}
	}

	pub async fn subscribe<B: Into<Broadcast>, T: Into<Track>>(
		&mut self,
		broadcast: B,
		track: T,
	) -> Result<TrackReader, Error> {
		self.subscribe_inner(broadcast.into(), track.into()).await
	}

	#[tracing::instrument("subscribe", skip_all, err, fields(broadcast=broadcast.name, track=track.name))]
	pub async fn subscribe_inner(&mut self, broadcast: Broadcast, track: Track) -> Result<TrackReader, Error> {
		let sub = self.init_subscribe(track);
		let mut stream = self.session.open(message::Stream::Subscribe).await?;

		self.start_subscribe(&mut stream, broadcast, &sub)
			.await
			.or_close(&mut stream)?; // wait for an OK before returning

		let mut this = self.clone();
		let track = sub.track.clone();

		runtime::spawn(async move {
			this.run_subscribe(&mut stream, sub).await.or_close(&mut stream).ok();
		});

		Ok(track)
	}

	fn init_subscribe(&mut self, track: Track) -> Subscribe {
		let id = self.next_id.fetch_add(1, atomic::Ordering::Relaxed);

		let (writer, reader) = track.produce();
		self.tracks.lock().insert(id, writer);

		Subscribe {
			id,
			track: reader,
			tracks: self.tracks.clone(),
		}
	}

	async fn start_subscribe(
		&mut self,
		stream: &mut Stream,
		broadcast: Broadcast,
		sub: &Subscribe,
	) -> Result<(), Error> {
		let request = message::Subscribe {
			id: sub.id,
			broadcast: broadcast.name.clone(),

			track: sub.track.name.clone(),
			priority: sub.track.priority,

			group_order: sub.track.group_order,
			group_expires: sub.track.group_expires,

			// TODO
			group_min: None,
			group_max: None,
		};

		stream.writer.encode(&request).await?;

		// TODO use the response to update the track
		let _response: message::Info = stream.reader.decode().await?;

		tracing::info!("ok");

		Ok(())
	}

	async fn run_subscribe(&mut self, stream: &mut Stream, sub: Subscribe) -> Result<(), Error> {
		loop {
			tokio::select! {
				res = stream.reader.decode_maybe::<message::GroupDrop>() => {
					// TODO expose updates to application
					// TODO use to detect gaps
					if res?.is_none() {
						return Ok(());
					}
				},
				res = sub.track.closed() => res?,
			};
		}
	}

	pub(super) async fn recv_announce(&mut self, stream: &mut Stream) -> Result<(), Error> {
		let announce = stream.reader.decode().await?;
		self.announced_run(stream, announce).await
	}

	#[tracing::instrument("announced", skip_all, err, fields(broadcast = announce.broadcast))]
	async fn announced_run(&mut self, stream: &mut Stream, announce: message::Announce) -> Result<(), Error> {
		// Serve the broadcast and add it to the announced queue.
		let broadcast = self.namespace(announce.broadcast)?;
		self.announced.push(broadcast.clone()).map_err(|_| Error::Cancel)?;

		// Send the OK message.
		let msg = message::AnnounceOk {};
		stream.writer.encode(&msg).await?;

		tracing::info!("ok");

		// Wait until the stream is closed.
		tokio::select! {
			res = stream.reader.closed() => res,
			res = broadcast.closed() => res.map_err(Into::into),
		}
	}

	pub(super) async fn recv_group(&mut self, stream: &mut Reader) -> Result<(), Error> {
		let group = stream.decode().await?;
		self.serve_group(stream, group).await
	}

	#[tracing::instrument("data", skip_all, err, fields(group = group.sequence))]
	async fn serve_group(&mut self, stream: &mut Reader, group: message::Group) -> Result<(), Error> {
		let mut group = self
			.tracks
			.lock()
			.get_mut(&group.subscribe)
			.ok_or(Error::NotFound)?
			.create_group(group.sequence)?;

		while let Some(frame) = stream.decode_maybe::<message::Frame>().await? {
			let mut frame = group.create_frame(frame.size)?;
			let mut remain = frame.size;

			while remain > 0 {
				let chunk = stream.read_chunk(remain).await?.ok_or(Error::WrongSize)?;

				remain = remain.checked_sub(chunk.len()).ok_or(Error::WrongSize)?;
				tracing::trace!(chunk = chunk.len(), remain, "chunk");

				frame.write_chunk(chunk)?;
			}
		}

		Ok(())
	}

	pub async fn closed(&self) -> Result<(), Error> {
		self.session.closed().await
	}
}

// Simple wrapper to remove on drop.
struct Subscribe {
	pub id: u64,
	pub track: TrackReader,
	tracks: Lock<HashMap<u64, TrackWriter>>,
}

impl Drop for Subscribe {
	fn drop(&mut self) {
		self.tracks.lock().remove(&self.id);
	}
}

// Simple wrapper to remove on drop.
struct Announce {
	pub broadcast: BroadcastWriter,
	pub router: RouterWriter<Track>,
	broadcasts: Lock<HashMap<String, BroadcastReader>>,
}

impl Drop for Announce {
	fn drop(&mut self) {
		self.broadcasts.lock().remove(&self.broadcast.name);
	}
}
