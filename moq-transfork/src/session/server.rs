use super::{OrClose, Stream};

use crate::{
	message,
	setup::{self, Extensions},
	MoqError, Publisher, Session, Subscriber,
};

pub struct Server {
	session: Session,
}

impl Server {
	pub fn new(session: web_transport::Session) -> Self {
		Self {
			session: Session::new(session),
		}
	}

	pub async fn publisher(self) -> Result<Publisher, MoqError> {
		let (publisher, _) = self.role(setup::Role::Publisher).await?;
		Ok(publisher.unwrap())
	}

	pub async fn subscriber(self) -> Result<Subscriber, MoqError> {
		let (_, subscriber) = self.role(setup::Role::Subscriber).await?;
		Ok(subscriber.unwrap())
	}

	/// Accept a session as both a publisher and subscriber.
	pub async fn both(self) -> Result<(Publisher, Subscriber), MoqError> {
		self.role(setup::Role::Both)
			.await
			.map(|(publisher, subscriber)| (publisher.unwrap(), subscriber.unwrap()))
	}

	/// Accept a session as either a publisher, subscriber, or both, as chosen by the client.
	pub async fn any(self) -> Result<(Option<Publisher>, Option<Subscriber>), MoqError> {
		self.role(setup::Role::Any).await
	}

	pub async fn role(mut self, role: setup::Role) -> Result<(Option<Publisher>, Option<Subscriber>), MoqError> {
		let mut stream = self.session.accept().await?;
		let kind = stream.reader.decode_silent().await?;

		if kind != message::Stream::Session {
			return Err(MoqError::UnexpectedStream(kind));
		}

		let role = Self::setup(&mut stream, role).await.or_close(&mut stream)?;

		Ok(Session::start(self.session, role, stream))
	}

	async fn setup(control: &mut Stream, server_role: setup::Role) -> Result<setup::Role, MoqError> {
		let client: setup::Client = control.reader.decode().await?;

		if !client.versions.contains(&setup::Version::FORK_00) {
			return Err(MoqError::Version(client.versions, [setup::Version::FORK_00].into()));
		}

		let client_role = client.extensions.get()?.unwrap_or_default();

		let role = server_role
			.downgrade(client_role)
			.ok_or(MoqError::RoleIncompatible(client_role, server_role))?;

		let mut extensions = Extensions::default();
		extensions.set(role)?;

		let server = setup::Server {
			version: setup::Version::FORK_00,
			extensions,
		};

		control.writer.encode(&server).await?;

		tracing::info!(version = ?server.version, ?role, "connected");

		Ok(server_role)
	}
}
