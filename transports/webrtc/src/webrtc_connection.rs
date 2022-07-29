// Copyright 2022 Parity Technologies (UK) Ltd.
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

use futures::{channel::oneshot, prelude::*, select};
use futures_timer::Delay;
use multihash::Hasher;
use rand::distributions::Alphanumeric;
use rand::{thread_rng, Rng};
use tinytemplate::TinyTemplate;
use webrtc::api::setting_engine::SettingEngine;
use webrtc::api::APIBuilder;
use webrtc::data_channel::data_channel_init::RTCDataChannelInit;
use webrtc::dtls_transport::dtls_fingerprint::RTCDtlsFingerprint;
use webrtc::dtls_transport::dtls_role::DTLSRole;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc_data::data_channel::DataChannel;
use webrtc_ice::network_type::NetworkType;
use webrtc_ice::udp_mux::UDPMux;
use webrtc_ice::udp_network::UDPNetwork;

use std::{net::SocketAddr, sync::Arc, time::Duration};

use crate::error::Error;
use crate::sdp;

pub(crate) struct WebRTCConnection {
    peer_connection: RTCPeerConnection,
}

impl WebRTCConnection {
    /// Creates a new WebRTC peer connection to the remote.
    ///
    /// # Panics
    ///
    /// Panics if the given address is not valid WebRTC dialing address.
    pub async fn connect(
        addr: SocketAddr,
        config: RTCConfiguration,
        udp_mux: Arc<dyn UDPMux + Send + Sync>,
        remote_fingerprint: &Fingerprint,
    ) -> Result<Self, Error> {
        // TODO: at least 128 bit of entropy
        let ufrag: String = thread_rng()
            .sample_iter(&Alphanumeric)
            .take(64)
            .map(char::from)
            .collect();
        let se = Self::setting_engine(udp_mux, &ufrag, addr.is_ipv4());
        let api = APIBuilder::new().with_setting_engine(se).build();

        let peer_connection = api.new_peer_connection(config).await?;

        // 1. OFFER
        let offer = peer_connection.create_offer(None).await?;
        log::debug!("OFFER: {:?}", offer.sdp);
        peer_connection.set_local_description(offer).await?;

        // 2. ANSWER
        // Set the remote description to the predefined SDP.
        let remote_ufrag = remote_fingerprint.to_ufrag();
        let server_session_description = render_description(
            sdp::SERVER_SESSION_DESCRIPTION,
            addr,
            remote_fingerprint,
            &remote_ufrag,
        );
        log::debug!("ANSWER: {:?}", server_session_description);
        let sdp = RTCSessionDescription::answer(server_session_description).unwrap();
        // NOTE: this will start the gathering of ICE candidates
        peer_connection.set_remote_description(sdp).await?;

        return Ok(Self { peer_connection });
    }

    pub async fn accept(
        addr: SocketAddr,
        config: RTCConfiguration,
        udp_mux: Arc<dyn UDPMux + Send + Sync>,
        our_fingerprint: &Fingerprint,
        remote_ufrag: &str,
    ) -> Result<Self, Error> {
        // Set both ICE user and password to our fingerprint because that's what the client is
        // expecting (see [`Self::connect`] "2. ANSWER").
        let ufrag = our_fingerprint.to_ufrag();
        let mut se = Self::setting_engine(udp_mux, &ufrag, addr.is_ipv4());
        {
            se.set_lite(true);
            se.disable_certificate_fingerprint_verification(true);
            // Act as a DTLS server (one which waits for a connection).
            //
            // NOTE: removing this seems to break DTLS setup (both sides send `ClientHello` messages,
            // but none end up responding).
            se.set_answering_dtls_role(DTLSRole::Server)?;
        }

        let api = APIBuilder::new().with_setting_engine(se).build();
        let peer_connection = api.new_peer_connection(config).await?;

        let client_session_description = render_description(
            sdp::CLIENT_SESSION_DESCRIPTION,
            addr,
            &Fingerprint::new_sha256("NONE".to_owned()), // certificate verification is disabled, so any value is okay.
            remote_ufrag,
        );
        log::debug!("OFFER: {:?}", client_session_description);
        let sdp = RTCSessionDescription::offer(client_session_description).unwrap();
        peer_connection.set_remote_description(sdp).await?;

        let answer = peer_connection.create_answer(None).await?;
        // Set the local description and start UDP listeners
        // Note: this will start the gathering of ICE candidates
        log::debug!("ANSWER: {:?}", answer.sdp);
        peer_connection.set_local_description(answer).await?;

        return Ok(Self { peer_connection });
    }

    pub async fn create_initial_upgrade_data_channel(
        &self,
        negotiated: Option<bool>,
    ) -> Result<Arc<DataChannel>, Error> {
        // Open a data channel to do Noise on top and verify the remote.
        let data_channel = self
            .peer_connection
            .create_data_channel(
                "data",
                Some(RTCDataChannelInit {
                    id: Some(1),
                    negotiated,
                    ..RTCDataChannelInit::default()
                }),
            )
            .await?;

        let (tx, mut rx) = oneshot::channel::<Arc<DataChannel>>();

        // Wait until the data channel is opened and detach it.
        crate::connection::register_data_channel_open_handler(data_channel, tx).await;
        select! {
            res = rx => match res {
                Ok(detached) => Ok(detached),
                Err(e) => return Err(Error::InternalError(e.to_string())),
            },
            _ = Delay::new(Duration::from_secs(10)).fuse() => return Err(Error::InternalError(
                "data channel opening took longer than 10 seconds (see logs)".into(),
            ))
        }
    }

    pub fn into_inner(self) -> RTCPeerConnection {
        self.peer_connection
    }

    pub async fn get_remote_fingerprint(&self) -> [u8; 32] {
        let cert_bytes = self
            .peer_connection
            .sctp()
            .transport()
            .get_remote_certificate()
            .await;
        let mut h = multihash::Sha2_256::default();
        h.update(&cert_bytes);
        let mut bytes: [u8; 32] = [0; 32];
        bytes.copy_from_slice(h.finalize());
        bytes
    }

    fn setting_engine(
        udp_mux: Arc<dyn UDPMux + Send + Sync>,
        ufrag: &str,
        is_ipv4: bool,
    ) -> SettingEngine {
        let mut se = SettingEngine::default();

        se.set_ice_credentials(ufrag.to_owned(), ufrag.to_owned());

        se.set_udp_network(UDPNetwork::Muxed(udp_mux.clone()));

        // Allow detaching data channels.
        se.detach_data_channels();

        // Set the desired network type.
        //
        // NOTE: if not set, a [`webrtc_ice::agent::Agent`] might pick a wrong local candidate
        // (e.g. IPv6 `[::1]` while dialing an IPv4 `10.11.12.13`).
        let network_type = if is_ipv4 {
            NetworkType::Udp4
        } else {
            NetworkType::Udp6
        };
        se.set_network_types(vec![network_type]);

        se
    }
}

const SHA256: &str = "sha-256";

pub(crate) struct Fingerprint(RTCDtlsFingerprint);

impl Fingerprint {
    /// Creates new `Fingerprint` w/ "sha-256" hash function.
    pub fn new_sha256(value: String) -> Self {
        Self(RTCDtlsFingerprint {
            algorithm: SHA256.to_owned(),
            value,
        })
    }

    /// Transforms this fingerprint into a ufrag.
    pub fn to_ufrag(&self) -> String {
        self.0.value.replace(':', "").to_lowercase()
    }

    /// Returns the lower-hex value, each byte separated by ":".
    /// E.g. "7D:E3:D8:3F:81:A6:80:59:2A:47:1E:6B:6A:BB:07:47:AB:D3:53:85:A8:09:3F:DF:E1:12:C1:EE:BB:6C:C6:AC"
    pub fn value(&self) -> String {
        self.0.value.clone()
    }

    /// Returns the algorithm used (e.g. "sha-256").
    /// See https://datatracker.ietf.org/doc/html/rfc8122#section-5
    pub fn algorithm(&self) -> String {
        self.0.algorithm.clone()
    }
}

impl<T> From<T> for Fingerprint
where
    T: IntoIterator,
    <T as IntoIterator>::Item: core::fmt::UpperHex,
{
    fn from(t: T) -> Self {
        let values: Vec<String> = t.into_iter().map(|x| format! {"{:02X}", x}).collect();
        Self(RTCDtlsFingerprint {
            algorithm: SHA256.to_owned(),
            value: values.join(":"),
        })
    }
}

impl AsRef<str> for Fingerprint {
    #[inline]
    fn as_ref(&self) -> &str {
        self.0.value.as_ref()
    }
}

/// Renders a [`TinyTemplate`] description using the provided arguments.
pub(crate) fn render_description(
    description: &str,
    addr: SocketAddr,
    fingerprint: &Fingerprint,
    ufrag: &str,
) -> String {
    let mut tt = TinyTemplate::new();
    tt.add_template("description", description).unwrap();

    let context = sdp::DescriptionContext {
        ip_version: {
            if addr.is_ipv4() {
                sdp::IpVersion::IP4
            } else {
                sdp::IpVersion::IP6
            }
        },
        target_ip: addr.ip(),
        target_port: addr.port(),
        fingerprint_algorithm: fingerprint.algorithm(),
        fingerprint_value: fingerprint.value(),
        // NOTE: ufrag is equal to pwd.
        ufrag: ufrag.to_owned(),
        pwd: ufrag.to_owned(),
    };
    tt.render("description", &context).unwrap()
}