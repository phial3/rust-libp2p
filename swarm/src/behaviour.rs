// Copyright 2019 Parity Technologies (UK) Ltd.
//
// Permission is hereby granted, free of charge, to any person obtaining a
// copy of this software and associated documentation files (the "Software"),
// to deal in the Software without restriction, including without limitation
// the rights to use, copy, modify, merge, publish, distribute, sublicense,
// and/or sell copies of the Software, and to permit persons to whom the
// Software is furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS
// OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
// FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

mod either;
mod external_addresses;
mod listen_addresses;
pub mod toggle;

pub use external_addresses::ExternalAddresses;
pub use listen_addresses::ListenAddresses;

use crate::dial_opts::DialOpts;
#[allow(deprecated)]
use crate::handler::IntoConnectionHandler;
use crate::{AddressRecord, AddressScore, DialError, THandler, THandlerInEvent, THandlerOutEvent};
use libp2p_core::{
    connection::ConnectionId, transport::ListenerId, ConnectedPoint, Endpoint, Multiaddr, PeerId,
};
use std::{task::Context, task::Poll};

/// A [`NetworkBehaviour`] defines the behaviour of the local node on the network.
///
/// In contrast to [`Transport`](libp2p_core::Transport) which defines **how** to send bytes on the
/// network, [`NetworkBehaviour`] defines **what** bytes to send and **to whom**.
///
/// Each protocol (e.g. `libp2p-ping`, `libp2p-identify` or `libp2p-kad`) implements
/// [`NetworkBehaviour`]. Multiple implementations of [`NetworkBehaviour`] can be composed into a
/// hierarchy of [`NetworkBehaviour`]s where parent implementations delegate to child
/// implementations. Finally the root of the [`NetworkBehaviour`] hierarchy is passed to
/// [`Swarm`](crate::Swarm) where it can then control the behaviour of the local node on a libp2p
/// network.
///
/// # Hierarchy of [`NetworkBehaviour`]
///
/// To compose multiple [`NetworkBehaviour`] implementations into a single [`NetworkBehaviour`]
/// implementation, potentially building a multi-level hierarchy of [`NetworkBehaviour`]s, one can
/// use one of the [`NetworkBehaviour`] combinators, and/or use the [`NetworkBehaviour`] derive
/// macro.
///
/// ## Combinators
///
/// [`NetworkBehaviour`] combinators wrap one or more [`NetworkBehaviour`] implementations and
/// implement [`NetworkBehaviour`] themselves. Example is the
/// [`Toggle`](crate::behaviour::toggle::Toggle) [`NetworkBehaviour`].
///
/// ``` rust
/// # use libp2p_swarm::dummy;
/// # use libp2p_swarm::behaviour::toggle::Toggle;
/// let my_behaviour = dummy::Behaviour;
/// let my_toggled_behaviour = Toggle::from(Some(my_behaviour));
/// ```
///
/// ## Custom [`NetworkBehaviour`] with the Derive Macro
///
/// One can derive [`NetworkBehaviour`] for a custom `struct` via the `#[derive(NetworkBehaviour)]`
/// proc macro re-exported by the `libp2p` crate. The macro generates a delegating `trait`
/// implementation for the custom `struct`. Each [`NetworkBehaviour`] trait method is simply
/// delegated to each `struct` member in the order the `struct` is defined. For example for
/// [`NetworkBehaviour::poll`] it will first poll the first `struct` member until it returns
/// [`Poll::Pending`] before moving on to later members. For [`NetworkBehaviour::addresses_of_peer`]
/// it will delegate to each `struct` member and return a concatenated array of all addresses
/// returned by the struct members.
///
/// Events ([`NetworkBehaviour::OutEvent`]) returned by each `struct` member are wrapped in a new
/// `enum` event, with an `enum` variant for each `struct` member. Users can define this event
/// `enum` themselves and provide the name to the derive macro via `#[behaviour(out_event =
/// "MyCustomOutEvent")]`. If the user does not specify an `out_event`, the derive macro generates
/// the event definition itself, naming it `<STRUCT_NAME>Event`.
///
/// The aforementioned conversion of each of the event types generated by the struct members to the
/// custom `out_event` is handled by [`From`] implementations which the user needs to define in
/// addition to the event `enum` itself.
///
/// ``` rust
/// # use libp2p_identify as identify;
/// # use libp2p_ping as ping;
/// # use libp2p_swarm_derive::NetworkBehaviour;
/// #[derive(NetworkBehaviour)]
/// #[behaviour(out_event = "Event")]
/// # #[behaviour(prelude = "libp2p_swarm::derive_prelude")]
/// struct MyBehaviour {
///   identify: identify::Behaviour,
///   ping: ping::Behaviour,
/// }
///
/// enum Event {
///   Identify(identify::Event),
///   Ping(ping::Event),
/// }
///
/// impl From<identify::Event> for Event {
///   fn from(event: identify::Event) -> Self {
///     Self::Identify(event)
///   }
/// }
///
/// impl From<ping::Event> for Event {
///   fn from(event: ping::Event) -> Self {
///     Self::Ping(event)
///   }
/// }
/// ```
pub trait NetworkBehaviour: 'static {
    /// Handler for all the protocols the network behaviour supports.
    #[allow(deprecated)]
    type ConnectionHandler: IntoConnectionHandler;

    /// Event generated by the `NetworkBehaviour` and that the swarm will report back.
    type OutEvent: Send + 'static;

    // /// Creates a new [`ConnectionHandler`] for a connection with a peer.
    // ///
    // /// Every time an incoming connection is opened, and every time another [`NetworkBehaviour`]
    // /// emitted a dial request, this method is called.
    // ///
    // /// The returned object is a handler for that specific connection, and will be moved to a
    // /// background task dedicated to that connection.
    // ///
    // /// The network behaviour (ie. the implementation of this trait) and the handlers it has spawned
    // /// (ie. the objects returned by `new_handler`) can communicate by passing messages. Messages
    // /// sent from the handler to the behaviour are injected with [`NetworkBehaviour::inject_event`],
    // /// and the behaviour can send a message to the handler by making [`NetworkBehaviour::poll`]
    // /// return [`NetworkBehaviourAction::NotifyHandler`].
    // ///
    // /// Note that the handler is returned to the [`NetworkBehaviour`] on connection failure and
    // /// connection closing.
    #[deprecated(
        since = "0.42.0",
        note = "Use one or more of `NetworkBehaviour::{handle_pending_inbound_connection,handle_established_inbound_connection,handle_pending_outbound_connection,handle_established_outbound_connection}` instead."
    )]
    fn new_handler(&mut self) -> Self::ConnectionHandler {
        panic!("You must implement `handle_established_inbound_connection` and `handle_established_outbound_connection`.")
    }

    /// Callback that is invoked for every new inbound connection.
    ///
    /// At this point in the connection lifecycle, only the remote's and our local address are known.
    /// We have also already allocated a [`ConnectionId`].
    ///
    /// Any error returned from this function will immediately abort the dial attempt.
    fn handle_pending_inbound_connection(
        &mut self,
        _connection_id: ConnectionId,
        _local_addr: &Multiaddr,
        _remote_addr: &Multiaddr,
    ) -> Result<(), Box<dyn std::error::Error + Send + 'static>> {
        Ok(())
    }

    /// Callback that is invoked for every established inbound connection.
    ///
    /// This is invoked once another peer has successfully dialed us.
    ///
    /// At this point, we have verified their [`PeerId`] and we know, which particular [`Multiaddr`] succeeded in the dial.
    /// In order to actually use this connection, this function must return a [`ConnectionHandler`].
    /// Returning an error will immediately close the connection.
    fn handle_established_inbound_connection(
        &mut self,
        peer: PeerId,
        _connection_id: ConnectionId,
        local_addr: &Multiaddr,
        remote_addr: &Multiaddr,
    ) -> Result<THandler<Self>, Box<dyn std::error::Error + Send + 'static>> {
        #[allow(deprecated)]
        Ok(self.new_handler().into_handler(
            &peer,
            &ConnectedPoint::Listener {
                local_addr: local_addr.clone(),
                send_back_addr: remote_addr.clone(),
            },
        ))
    }

    /// Callback that is invoked for every outbound connection attempt.
    ///
    /// We have access to:
    ///
    /// - The [`PeerId`], if known. Remember that we can dial without a [`PeerId`].
    /// - All addresses passes to [`DialOpts`] are passed in here too.
    /// - The effective [`Role`](Endpoint) of this peer in the dial attempt. Typically, this is set to [`Endpoint::Dialer`] except if we are attempting a hole-punch.
    /// - The [`ConnectionId`] identifying the future connection resulting from this dial, if successful.
    ///
    /// Any error returned from this function will immediately abort the dial attempt.
    fn handle_pending_outbound_connection(
        &mut self,
        maybe_peer: Option<PeerId>,
        _addresses: &[Multiaddr],
        _effective_role: Endpoint,
        _connection_id: ConnectionId,
    ) -> Result<Vec<Multiaddr>, Box<dyn std::error::Error + Send + 'static>> {
        #[allow(deprecated)]
        if let Some(peer_id) = maybe_peer {
            Ok(self.addresses_of_peer(&peer_id))
        } else {
            Ok(vec![])
        }
    }

    /// Callback that is invoked for every established outbound connection.
    ///
    /// This is invoked once we have successfully dialed a peer.
    /// At this point, we have verified their [`PeerId`] and we know, which particular [`Multiaddr`] succeeded in the dial.
    /// In order to actually use this connection, this function must return a [`ConnectionHandler`].
    /// Returning an error will immediately close the connection.
    fn handle_established_outbound_connection(
        &mut self,
        peer: PeerId,
        addr: &Multiaddr,
        role_override: Endpoint,
        _connection_id: ConnectionId,
    ) -> Result<THandler<Self>, Box<dyn std::error::Error + Send + 'static>> {
        #[allow(deprecated)]
        Ok(self.new_handler().into_handler(
            &peer,
            &ConnectedPoint::Dialer {
                address: addr.clone(),
                role_override,
            },
        ))
    }

    /// Addresses that this behaviour is aware of for this specific peer, and that may allow
    /// reaching the peer.
    ///
    /// The addresses will be tried in the order returned by this function, which means that they
    /// should be ordered by decreasing likelihood of reachability. In other words, the first
    /// address should be the most likely to be reachable.
    #[deprecated(note = "Use `NetworkBehaviour::handle_pending_outbound_connection` instead.")]
    fn addresses_of_peer(&mut self, _: &PeerId) -> Vec<Multiaddr> {
        vec![]
    }

    /// Informs the behaviour about an event from the [`Swarm`](crate::Swarm).
    fn on_swarm_event(&mut self, event: FromSwarm<Self::ConnectionHandler>);

    /// Informs the behaviour about an event generated by the [`ConnectionHandler`] dedicated to the
    /// peer identified by `peer_id`. for the behaviour.
    ///
    /// The [`PeerId`] is guaranteed to be in a connected state. In other words,
    /// [`FromSwarm::ConnectionEstablished`] has previously been received with this [`PeerId`].
    fn on_connection_handler_event(
        &mut self,
        _peer_id: PeerId,
        _connection_id: ConnectionId,
        _event: THandlerOutEvent<Self>,
    ) {
    }

    /// Polls for things that swarm should do.
    ///
    /// This API mimics the API of the `Stream` trait. The method may register the current task in
    /// order to wake it up at a later point in time.
    fn poll(
        &mut self,
        cx: &mut Context<'_>,
        params: &mut impl PollParameters,
    ) -> Poll<NetworkBehaviourAction<Self::OutEvent, THandlerInEvent<Self>>>;
}

/// Parameters passed to `poll()`, that the `NetworkBehaviour` has access to.
pub trait PollParameters {
    /// Iterator returned by [`supported_protocols`](PollParameters::supported_protocols).
    type SupportedProtocolsIter: ExactSizeIterator<Item = Vec<u8>>;
    /// Iterator returned by [`listened_addresses`](PollParameters::listened_addresses).
    type ListenedAddressesIter: ExactSizeIterator<Item = Multiaddr>;
    /// Iterator returned by [`external_addresses`](PollParameters::external_addresses).
    type ExternalAddressesIter: ExactSizeIterator<Item = AddressRecord>;

    /// Returns the list of protocol the behaviour supports when a remote negotiates a protocol on
    /// an inbound substream.
    ///
    /// The iterator's elements are the ASCII names as reported on the wire.
    ///
    /// Note that the list is computed once at initialization and never refreshed.
    fn supported_protocols(&self) -> Self::SupportedProtocolsIter;

    /// Returns the list of the addresses we're listening on.
    #[deprecated(
        since = "0.42.0",
        note = "Use `libp2p_swarm::ListenAddresses` instead."
    )]
    fn listened_addresses(&self) -> Self::ListenedAddressesIter;

    /// Returns the list of the addresses nodes can use to reach us.
    #[deprecated(
        since = "0.42.0",
        note = "Use `libp2p_swarm::ExternalAddresses` instead."
    )]
    fn external_addresses(&self) -> Self::ExternalAddressesIter;

    /// Returns the peer id of the local node.
    #[deprecated(
        since = "0.42.0",
        note = "Pass the node's `PeerId` into the behaviour instead."
    )]
    fn local_peer_id(&self) -> &PeerId;
}

/// An action that a [`NetworkBehaviour`] can trigger in the [`Swarm`]
/// in whose context it is executing.
///
/// [`Swarm`]: super::Swarm
//
// Note: `TInEvent` is needed to be able to implement
// [`NetworkBehaviourAction::map_in`], mapping the handler `InEvent` leaving the
// handler itself untouched.
#[derive(Debug)]
pub enum NetworkBehaviourAction<TOutEvent, TInEvent> {
    /// Instructs the `Swarm` to return an event when it is being polled.
    GenerateEvent(TOutEvent),

    /// Instructs the swarm to start a dial.
    Dial { opts: DialOpts, id: ConnectionId },

    /// Instructs the `Swarm` to send an event to the handler dedicated to a
    /// connection with a peer.
    ///
    /// If the `Swarm` is connected to the peer, the message is delivered to the
    /// [`ConnectionHandler`] instance identified by the peer ID and connection ID.
    ///
    /// If the specified connection no longer exists, the event is silently dropped.
    ///
    /// Typically the connection ID given is the same as the one passed to
    /// [`NetworkBehaviour::on_connection_handler_event`], i.e. whenever the behaviour wishes to
    /// respond to a request on the same connection (and possibly the same
    /// substream, as per the implementation of [`ConnectionHandler`]).
    ///
    /// Note that even if the peer is currently connected, connections can get closed
    /// at any time and thus the event may not reach a handler.
    NotifyHandler {
        /// The peer for whom a [`ConnectionHandler`] should be notified.
        peer_id: PeerId,
        /// The options w.r.t. which connection handler to notify of the event.
        handler: NotifyHandler,
        /// The event to send.
        event: TInEvent,
    },

    /// Informs the `Swarm` about an address observed by a remote for
    /// the local node by which the local node is supposedly publicly
    /// reachable.
    ///
    /// It is advisable to issue `ReportObservedAddr` actions at a fixed frequency
    /// per node. This way address information will be more accurate over time
    /// and individual outliers carry less weight.
    ReportObservedAddr {
        /// The observed address of the local node.
        address: Multiaddr,
        /// The score to associate with this observation, i.e.
        /// an indicator for the trusworthiness of this address
        /// relative to other observed addresses.
        score: AddressScore,
    },

    /// Instructs the `Swarm` to initiate a graceful close of one or all connections
    /// with the given peer.
    ///
    /// Note: Closing a connection via
    /// [`NetworkBehaviourAction::CloseConnection`] does not inform the
    /// corresponding [`ConnectionHandler`].
    /// Closing a connection via a [`ConnectionHandler`] can be done
    /// either in a collaborative manner across [`ConnectionHandler`]s
    /// with [`ConnectionHandler::connection_keep_alive`] or directly with
    /// [`ConnectionHandlerEvent::Close`](crate::ConnectionHandlerEvent::Close).
    CloseConnection {
        /// The peer to disconnect.
        peer_id: PeerId,
        /// Whether to close a specific or all connections to the given peer.
        connection: CloseConnection,
    },
}

impl<TOutEvent, TInEvent> NetworkBehaviourAction<TOutEvent, TInEvent> {
    /// TODO: Docs
    pub fn dial(opts: impl Into<DialOpts>) -> (Self, ConnectionId) {
        let id = ConnectionId::next();

        let action = Self::Dial {
            opts: opts.into(),
            id,
        };

        (action, id)
    }
}

impl<TOutEvent, TInEventOld> NetworkBehaviourAction<TOutEvent, TInEventOld> {
    /// Map the handler event.
    pub fn map_in<TInEventNew>(
        self,
        f: impl FnOnce(TInEventOld) -> TInEventNew,
    ) -> NetworkBehaviourAction<TOutEvent, TInEventNew> {
        match self {
            NetworkBehaviourAction::GenerateEvent(e) => NetworkBehaviourAction::GenerateEvent(e),
            NetworkBehaviourAction::Dial { opts, id } => NetworkBehaviourAction::Dial { opts, id },
            NetworkBehaviourAction::NotifyHandler {
                peer_id,
                handler,
                event,
            } => NetworkBehaviourAction::NotifyHandler {
                peer_id,
                handler,
                event: f(event),
            },
            NetworkBehaviourAction::ReportObservedAddr { address, score } => {
                NetworkBehaviourAction::ReportObservedAddr { address, score }
            }
            NetworkBehaviourAction::CloseConnection {
                peer_id,
                connection,
            } => NetworkBehaviourAction::CloseConnection {
                peer_id,
                connection,
            },
        }
    }
}

impl<TOutEvent, TInEvent> NetworkBehaviourAction<TOutEvent, TInEvent> {
    /// Map the event the swarm will return.
    pub fn map_out<E>(self, f: impl FnOnce(TOutEvent) -> E) -> NetworkBehaviourAction<E, TInEvent> {
        match self {
            NetworkBehaviourAction::GenerateEvent(e) => NetworkBehaviourAction::GenerateEvent(f(e)),
            NetworkBehaviourAction::Dial { opts, id } => NetworkBehaviourAction::Dial { opts, id },
            NetworkBehaviourAction::NotifyHandler {
                peer_id,
                handler,
                event,
            } => NetworkBehaviourAction::NotifyHandler {
                peer_id,
                handler,
                event,
            },
            NetworkBehaviourAction::ReportObservedAddr { address, score } => {
                NetworkBehaviourAction::ReportObservedAddr { address, score }
            }
            NetworkBehaviourAction::CloseConnection {
                peer_id,
                connection,
            } => NetworkBehaviourAction::CloseConnection {
                peer_id,
                connection,
            },
        }
    }
}

/// The options w.r.t. which connection handler to notify of an event.
#[derive(Debug, Clone)]
pub enum NotifyHandler {
    /// Notify a particular connection handler.
    One(ConnectionId),
    /// Notify an arbitrary connection handler.
    Any,
}

/// The options which connections to close.
#[derive(Debug, Clone)]
pub enum CloseConnection {
    /// Disconnect a particular connection.
    One(ConnectionId),
    /// Disconnect all connections.
    All,
}

impl Default for CloseConnection {
    fn default() -> Self {
        CloseConnection::All
    }
}

/// Enumeration with the list of the possible events
/// to pass to [`on_swarm_event`](NetworkBehaviour::on_swarm_event).
#[allow(deprecated)]
pub enum FromSwarm<'a, Handler: IntoConnectionHandler> {
    /// Informs the behaviour about a newly established connection to a peer.
    ConnectionEstablished(ConnectionEstablished<'a>),
    /// Informs the behaviour about a closed connection to a peer.
    ///
    /// This event is always paired with an earlier
    /// [`FromSwarm::ConnectionEstablished`] with the same peer ID, connection ID
    /// and endpoint.
    ConnectionClosed(ConnectionClosed<'a, Handler>),
    /// Informs the behaviour that the [`ConnectedPoint`] of an existing
    /// connection has changed.
    AddressChange(AddressChange<'a>),
    /// Informs the behaviour that the dial to a known
    /// or unknown node failed.
    DialFailure(DialFailure<'a>),
    /// Informs the behaviour that an error
    /// happened on an incoming connection during its initial handshake.
    ///
    /// This can include, for example, an error during the handshake of the encryption layer, or the
    /// connection unexpectedly closed.
    ListenFailure(ListenFailure<'a>),
    /// Informs the behaviour that a new listener was created.
    NewListener(NewListener),
    /// Informs the behaviour that we have started listening on a new multiaddr.
    NewListenAddr(NewListenAddr<'a>),
    /// Informs the behaviour that a multiaddr
    /// we were listening on has expired,
    /// which means that we are no longer listening on it.
    ExpiredListenAddr(ExpiredListenAddr<'a>),
    /// Informs the behaviour that a listener experienced an error.
    ListenerError(ListenerError<'a>),
    /// Informs the behaviour that a listener closed.
    ListenerClosed(ListenerClosed<'a>),
    /// Informs the behaviour that we have discovered a new external address for us.
    NewExternalAddr(NewExternalAddr<'a>),
    /// Informs the behaviour that an external address was removed.
    ExpiredExternalAddr(ExpiredExternalAddr<'a>),
}

/// [`FromSwarm`] variant that informs the behaviour about a newly established connection to a peer.
#[derive(Clone, Copy)]
pub struct ConnectionEstablished<'a> {
    pub peer_id: PeerId,
    pub connection_id: ConnectionId,
    pub endpoint: &'a ConnectedPoint,
    pub failed_addresses: &'a [Multiaddr],
    pub other_established: usize,
}

/// [`FromSwarm`] variant that informs the behaviour about a closed connection to a peer.
///
/// This event is always paired with an earlier
/// [`FromSwarm::ConnectionEstablished`] with the same peer ID, connection ID
/// and endpoint.
#[allow(deprecated)]
pub struct ConnectionClosed<'a, Handler: IntoConnectionHandler> {
    pub peer_id: PeerId,
    pub connection_id: ConnectionId,
    pub endpoint: &'a ConnectedPoint,
    pub handler: <Handler as IntoConnectionHandler>::Handler,
    pub remaining_established: usize,
}

/// [`FromSwarm`] variant that informs the behaviour that the [`ConnectedPoint`] of an existing
/// connection has changed.
#[derive(Clone, Copy)]
pub struct AddressChange<'a> {
    pub peer_id: PeerId,
    pub connection_id: ConnectionId,
    pub old: &'a ConnectedPoint,
    pub new: &'a ConnectedPoint,
}

/// [`FromSwarm`] variant that informs the behaviour that the dial to a known
/// or unknown node failed.
#[derive(Clone, Copy)]
pub struct DialFailure<'a> {
    pub peer_id: Option<PeerId>,
    pub error: &'a DialError,
    pub id: ConnectionId,
}

/// [`FromSwarm`] variant that informs the behaviour that an error
/// happened on an incoming connection during its initial handshake.
///
/// This can include, for example, an error during the handshake of the encryption layer, or the
/// connection unexpectedly closed.
#[derive(Clone, Copy)]
pub struct ListenFailure<'a> {
    pub local_addr: &'a Multiaddr,
    pub send_back_addr: &'a Multiaddr,
    pub id: ConnectionId,
}

/// [`FromSwarm`] variant that informs the behaviour that a new listener was created.
#[derive(Clone, Copy)]
pub struct NewListener {
    pub listener_id: ListenerId,
}

/// [`FromSwarm`] variant that informs the behaviour
/// that we have started listening on a new multiaddr.
#[derive(Clone, Copy)]
pub struct NewListenAddr<'a> {
    pub listener_id: ListenerId,
    pub addr: &'a Multiaddr,
}

/// [`FromSwarm`] variant that informs the behaviour that a multiaddr
/// we were listening on has expired,
/// which means that we are no longer listening on it.
#[derive(Clone, Copy)]
pub struct ExpiredListenAddr<'a> {
    pub listener_id: ListenerId,
    pub addr: &'a Multiaddr,
}

/// [`FromSwarm`] variant that informs the behaviour that a listener experienced an error.
#[derive(Clone, Copy)]
pub struct ListenerError<'a> {
    pub listener_id: ListenerId,
    pub err: &'a (dyn std::error::Error + 'static),
}

/// [`FromSwarm`] variant that informs the behaviour that a listener closed.
#[derive(Clone, Copy)]
pub struct ListenerClosed<'a> {
    pub listener_id: ListenerId,
    pub reason: Result<(), &'a std::io::Error>,
}

/// [`FromSwarm`] variant that informs the behaviour
/// that we have discovered a new external address for us.
#[derive(Clone, Copy)]
pub struct NewExternalAddr<'a> {
    pub addr: &'a Multiaddr,
}

/// [`FromSwarm`] variant that informs the behaviour that an external address was removed.
#[derive(Clone, Copy)]
pub struct ExpiredExternalAddr<'a> {
    pub addr: &'a Multiaddr,
}

#[allow(deprecated)]
impl<'a, Handler: IntoConnectionHandler> FromSwarm<'a, Handler> {
    fn map_handler<NewHandler>(
        self,
        map_handler: impl FnOnce(
            <Handler as IntoConnectionHandler>::Handler,
        ) -> <NewHandler as IntoConnectionHandler>::Handler,
    ) -> FromSwarm<'a, NewHandler>
    where
        NewHandler: IntoConnectionHandler,
    {
        self.maybe_map_handler(|h| Some(map_handler(h)))
            .expect("To return Some as all closures return Some.")
    }

    fn maybe_map_handler<NewHandler>(
        self,
        map_handler: impl FnOnce(
            <Handler as IntoConnectionHandler>::Handler,
        ) -> Option<<NewHandler as IntoConnectionHandler>::Handler>,
    ) -> Option<FromSwarm<'a, NewHandler>>
    where
        NewHandler: IntoConnectionHandler,
    {
        match self {
            FromSwarm::ConnectionClosed(ConnectionClosed {
                peer_id,
                connection_id,
                endpoint,
                handler,
                remaining_established,
            }) => Some(FromSwarm::ConnectionClosed(ConnectionClosed {
                peer_id,
                connection_id,
                endpoint,
                handler: map_handler(handler)?,
                remaining_established,
            })),
            FromSwarm::ConnectionEstablished(ConnectionEstablished {
                peer_id,
                connection_id,
                endpoint,
                failed_addresses,
                other_established,
            }) => Some(FromSwarm::ConnectionEstablished(ConnectionEstablished {
                peer_id,
                connection_id,
                endpoint,
                failed_addresses,
                other_established,
            })),
            FromSwarm::AddressChange(AddressChange {
                peer_id,
                connection_id,
                old,
                new,
            }) => Some(FromSwarm::AddressChange(AddressChange {
                peer_id,
                connection_id,
                old,
                new,
            })),
            FromSwarm::DialFailure(DialFailure { peer_id, error, id }) => {
                Some(FromSwarm::DialFailure(DialFailure { peer_id, error, id }))
            }
            FromSwarm::ListenFailure(ListenFailure {
                local_addr,
                send_back_addr,
                id,
            }) => Some(FromSwarm::ListenFailure(ListenFailure {
                local_addr,
                send_back_addr,
                id,
            })),
            FromSwarm::NewListener(NewListener { listener_id }) => {
                Some(FromSwarm::NewListener(NewListener { listener_id }))
            }
            FromSwarm::NewListenAddr(NewListenAddr { listener_id, addr }) => {
                Some(FromSwarm::NewListenAddr(NewListenAddr {
                    listener_id,
                    addr,
                }))
            }
            FromSwarm::ExpiredListenAddr(ExpiredListenAddr { listener_id, addr }) => {
                Some(FromSwarm::ExpiredListenAddr(ExpiredListenAddr {
                    listener_id,
                    addr,
                }))
            }
            FromSwarm::ListenerError(ListenerError { listener_id, err }) => {
                Some(FromSwarm::ListenerError(ListenerError { listener_id, err }))
            }
            FromSwarm::ListenerClosed(ListenerClosed {
                listener_id,
                reason,
            }) => Some(FromSwarm::ListenerClosed(ListenerClosed {
                listener_id,
                reason,
            })),
            FromSwarm::NewExternalAddr(NewExternalAddr { addr }) => {
                Some(FromSwarm::NewExternalAddr(NewExternalAddr { addr }))
            }
            FromSwarm::ExpiredExternalAddr(ExpiredExternalAddr { addr }) => {
                Some(FromSwarm::ExpiredExternalAddr(ExpiredExternalAddr { addr }))
            }
        }
    }
}
