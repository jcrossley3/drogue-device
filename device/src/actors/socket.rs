use super::wifi::*;
use crate::{
    kernel::actor::Address,
    traits::{
        ip::{IpProtocol, SocketAddress},
        tcp::{TcpError, TcpSocket, TcpStack},
    },
};

use core::future::Future;

/// A Socket type for connecting to a network endpoint + sending and receiving data.
#[derive(Clone, Copy)]
pub struct Socket<'a, A>
where
    A: Adapter + 'static,
{
    address: Address<'a, AdapterActor<A>>,
    handle: A::SocketHandle,
}

impl<'a, A> Socket<'a, A>
where
    A: Adapter + 'static,
{
    pub fn new(address: Address<'a, AdapterActor<A>>, handle: A::SocketHandle) -> Socket<'a, A> {
        Self { address, handle }
    }
}

impl<'a, A> TcpSocket for Socket<'a, A>
where
    A: Adapter + 'static,
{
    #[rustfmt::skip]
    type ConnectFuture<'m> where 'a: 'm, A: 'm =  impl Future<Output = Result<(), TcpError>>;
    fn connect<'m>(&'m mut self, proto: IpProtocol, dst: SocketAddress) -> Self::ConnectFuture<'m> {
        async move { self.address.connect(self.handle, proto, dst).await }
    }

    #[rustfmt::skip]
    type WriteFuture<'m> where 'a: 'm, A: 'm = impl Future<Output = Result<usize, TcpError>>;
    fn write<'m>(&'m mut self, buf: &'m [u8]) -> Self::WriteFuture<'m> {
        async move { self.address.write(self.handle, buf).await }
    }

    #[rustfmt::skip]
    type ReadFuture<'m> where 'a: 'm, A: 'm = impl Future<Output = Result<usize, TcpError>>;
    fn read<'m>(&'m mut self, buf: &'m mut [u8]) -> Self::ReadFuture<'m> {
        async move { self.address.read(self.handle, buf).await }
    }

    #[rustfmt::skip]
    type CloseFuture<'m> where 'a: 'm, A: 'm = impl Future<Output = ()>;
    fn close<'m>(&'m mut self) -> Self::CloseFuture<'m> {
        async move { self.address.close(self.handle).await }
    }
}

#[cfg(feature = "tls")]
mod tls {
    use super::{Adapter, Socket};
    use crate::traits::{
        ip::{IpProtocol, SocketAddress},
        tcp::{TcpError, TcpSocket},
    };
    use core::future::Future;
    use drogue_tls::{AsyncRead, AsyncWrite, TlsCipherSuite, TlsConfig, TlsConnection, TlsError};
    use rand_core::{CryptoRng, RngCore};

    impl<'a, T> AsyncRead for Socket<'a, T>
    where
        T: Adapter + 'static,
    {
        #[rustfmt::skip]
        type ReadFuture<'m> where Self: 'm = impl Future<Output = Result<usize, TlsError>> + 'm;
        fn read<'m>(&'m mut self, buf: &'m mut [u8]) -> Self::ReadFuture<'m> {
            async move {
                Ok(TcpSocket::read(self, buf)
                    .await
                    .map_err(|_| TlsError::IoError)?)
            }
        }
    }

    impl<'a, T> AsyncWrite for Socket<'a, T>
    where
        T: Adapter + 'static,
    {
        #[rustfmt::skip]
        type WriteFuture<'m> where Self: 'm = impl Future<Output = Result<usize, TlsError>> + 'm;
        fn write<'m>(&'m mut self, buf: &'m [u8]) -> Self::WriteFuture<'m> {
            async move {
                Ok(TcpSocket::write(self, buf)
                    .await
                    .map_err(|_| TlsError::IoError)?)
            }
        }
    }

    enum State<'a, S, RNG, CipherSuite, const FRAME_BUF_LEN: usize>
    where
        S: TcpSocket + AsyncWrite + AsyncRead + 'static,
        RNG: CryptoRng + RngCore + 'static,
        CipherSuite: TlsCipherSuite + 'static,
    {
        New(TlsConfig<'a, CipherSuite>, RNG, S),
        Connected(TlsConnection<'a, RNG, S, CipherSuite, FRAME_BUF_LEN>),
    }

    pub struct TlsSocket<'a, S, RNG, CipherSuite, const FRAME_BUF_LEN: usize>
    where
        S: TcpSocket + AsyncWrite + AsyncRead + 'static,
        RNG: CryptoRng + RngCore + 'static,
        CipherSuite: TlsCipherSuite + 'static,
    {
        state: Option<State<'a, S, RNG, CipherSuite, FRAME_BUF_LEN>>,
    }

    impl<'a, S, RNG, CipherSuite, const FRAME_BUF_LEN: usize>
        TlsSocket<'a, S, RNG, CipherSuite, FRAME_BUF_LEN>
    where
        S: TcpSocket + AsyncWrite + AsyncRead + 'static,
        RNG: CryptoRng + RngCore + 'static,
        CipherSuite: TlsCipherSuite + 'static,
    {
        pub fn wrap(socket: S, config: TlsConfig<'a, CipherSuite>, rng: RNG) -> Self {
            Self {
                state: Some(State::New(config, rng, socket)),
            }
        }
    }

    impl<'a, S, RNG, CipherSuite, const FRAME_BUF_LEN: usize> TcpSocket
        for TlsSocket<'a, S, RNG, CipherSuite, FRAME_BUF_LEN>
    where
        S: TcpSocket + AsyncWrite + AsyncRead + 'static,
        RNG: CryptoRng + RngCore + 'static,
        CipherSuite: TlsCipherSuite + 'static,
    {
        #[rustfmt::skip]
        type ConnectFuture<'m> where 'a: 'm, S: 'm, RNG: 'm, CipherSuite: 'm =  impl Future<Output = Result<(), TcpError>>;
        fn connect<'m>(
            &'m mut self,
            proto: IpProtocol,
            dst: SocketAddress,
        ) -> Self::ConnectFuture<'m> {
            async move {
                match self.state.take() {
                    Some(State::New(config, rng, mut socket)) => {
                        match socket.connect(proto, dst).await {
                            Ok(_) => {
                                info!("TCP connection opened");
                                let mut tls: TlsConnection<'a, RNG, S, CipherSuite, FRAME_BUF_LEN> =
                                    TlsConnection::new(config, rng, socket);
                                match tls.open().await {
                                    Ok(_) => {
                                        info!("TLS connection opened");
                                        self.state.replace(State::Connected(tls));
                                        Ok(())
                                    }
                                    Err(e) => {
                                        info!("TLS connection failed: {:?}", e);
                                        let (config, rng, socket) = tls.free();
                                        self.state.replace(State::New(config, rng, socket));
                                        Err(TcpError::ConnectError)
                                    }
                                }
                            }
                            Err(e) => {
                                info!("TCP connection failed: {:?}", e);
                                self.state.replace(State::New(config, rng, socket));
                                Err(e)
                            }
                        }
                    }
                    Some(other) => {
                        self.state.replace(other);
                        Err(TcpError::ConnectError)
                    }
                    None => Err(TcpError::SocketClosed),
                }
            }
        }

        #[rustfmt::skip]
        type WriteFuture<'m> where 'a: 'm, RNG: 'm, CipherSuite: 'm = impl Future<Output = Result<usize, TcpError>>;
        fn write<'m>(&'m mut self, buf: &'m [u8]) -> Self::WriteFuture<'m> {
            async move {
                match self.state.take() {
                    Some(State::Connected(mut session)) => {
                        let result = session.write(buf).await.map_err(|_| TcpError::WriteError);
                        self.state.replace(State::Connected(session));
                        result
                    }
                    Some(other) => {
                        self.state.replace(other);
                        Err(TcpError::SocketClosed)
                    }
                    None => Err(TcpError::SocketClosed),
                }
            }
        }

        #[rustfmt::skip]
        type ReadFuture<'m> where 'a: 'm, RNG: 'm, CipherSuite: 'm = impl Future<Output = Result<usize, TcpError>>;
        fn read<'m>(&'m mut self, buf: &'m mut [u8]) -> Self::ReadFuture<'m> {
            async move {
                match self.state.take() {
                    Some(State::Connected(mut session)) => {
                        let result = session.read(buf).await.map_err(|_| TcpError::ReadError);
                        self.state.replace(State::Connected(session));
                        result
                    }
                    Some(other) => {
                        self.state.replace(other);
                        Err(TcpError::SocketClosed)
                    }
                    None => Err(TcpError::SocketClosed),
                }
            }
        }

        #[rustfmt::skip]
        type CloseFuture<'m> where 'a: 'm, RNG: 'm, CipherSuite: 'm = impl Future<Output = ()>;
        fn close<'m>(&'m mut self) -> Self::CloseFuture<'m> {
            async move {
                match self.state.take() {
                    Some(State::Connected(session)) => {
                        // TODO: Send TLS alert
                        let (_, _, mut socket) = session.free();
                        socket.close().await;
                    }
                    Some(State::New(_, _, mut socket)) => {
                        socket.close().await;
                    }
                    None => {}
                }
            }
        }
    }
}

#[cfg(feature = "tls")]
pub use tls::*;
