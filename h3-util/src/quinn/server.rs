use hyper::body::Bytes;

use crate::server::H3Acceptor;

async fn select_conn(
    incoming: &h3_quinn::Endpoint,
    tasks: &mut tokio::task::JoinSet<Result<h3_quinn::Connection, crate::Error>>,
) -> SelectOutputConn {
    tracing::trace!("select_conn");

    let incoming_stream_future = async {
        tracing::trace!("endpoint waiting accept");
        match incoming.accept().await {
            Some(i) => {
                tracing::trace!("endpoint accept incoming conn");
                SelectOutputConn::NewIncoming(i)
            }
            None => SelectOutputConn::Done, // shutdown.
        }
    };
    if tasks.is_empty() {
        tracing::trace!("endpoint wait for new incoming");
        return incoming_stream_future.await;
    }
    tokio::select! {
        stream = incoming_stream_future => stream,
        accept = tasks.join_next() => {
            match accept.expect("JoinSet should never end") {
                Ok(conn) => {
                    match conn {
                        Ok(conn2) => {
                            SelectOutputConn::NewConn(conn2)
                        },
                        Err(e) => SelectOutputConn::ConnErr(e)
                    }
                },
                Err(e) => SelectOutputConn::ConnErr(e.into()),
            }
        }
    }
}

#[allow(clippy::large_enum_variant)]
enum SelectOutputConn {
    NewIncoming(h3_quinn::quinn::Incoming),
    NewConn(h3_quinn::Connection),
    ConnErr(crate::Error),
    Done,
}

pub struct H3QuinnAcceptor {
    ep: h3_quinn::Endpoint,
    tasks: tokio::task::JoinSet<Result<h3_quinn::Connection, crate::Error>>,
}

impl H3QuinnAcceptor {
    pub fn new(ep: h3_quinn::Endpoint) -> Self {
        Self {
            ep,
            tasks: Default::default(),
        }
    }
}

impl H3Acceptor for H3QuinnAcceptor {
    type CONN = h3_quinn::Connection;
    type OS = h3_quinn::OpenStreams;
    type SS = h3_quinn::SendStream<Bytes>;
    type RS = h3_quinn::RecvStream;
    type BS = h3_quinn::BidiStream<Bytes>;

    async fn accept(&mut self) -> Result<Option<Self::CONN>, crate::Error> {
        loop {
            match select_conn(&self.ep, &mut self.tasks).await {
                SelectOutputConn::NewIncoming(incoming) => {
                    tracing::trace!("poll conn new incoming");
                    self.tasks.spawn(async move {
                        let conn = incoming.await?;
                        let conn = h3_quinn::Connection::new(conn);
                        tracing::trace!("New incoming conn.");
                        Ok(conn)
                    });
                }
                SelectOutputConn::NewConn(connection) => {
                    return Ok(Some(connection));
                }
                SelectOutputConn::ConnErr(error) => {
                    // continue on error
                    tracing::warn!("conn error, ignore: {}", error);
                }
                SelectOutputConn::Done => {
                    return Ok(None);
                }
            }
        }
    }
}
