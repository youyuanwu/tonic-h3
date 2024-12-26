type H3ConnQuinn = crate::H3Conn<h3_quinn::Connection>;

/// incoming connections from quinn endpoint.
pub fn incoming_conn_quinn(
    mut acceptor: h3_quinn::Endpoint,
) -> impl futures::Stream<Item = Result<H3ConnQuinn, crate::Error>> {
    async_stream::try_stream! {

        let mut tasks = tokio::task::JoinSet::<Result<H3ConnQuinn, crate::Error>>::new();

        loop {
            match select_conn(&mut acceptor, &mut tasks).await {
                SelectOutputConn::NewIncoming(incoming) => {
                    tracing::debug!("poll conn new incoming");
                    tasks.spawn(async move {
                        let conn = incoming.await?;
                        let conn = h3::server::Connection::new(h3_quinn::Connection::new(conn))
                            .await
                            .map_err(crate::Error::from)?;
                        tracing::debug!("New incoming conn.");
                        Ok(conn)
                    });
                },
                SelectOutputConn::NewConn(connection) => {
                    yield connection;
                },
                SelectOutputConn::ConnErr(error) => {
                    // continue on error
                    tracing::debug!("conn error, ignore: {}" , error);
                },
                SelectOutputConn::Done => {
                    break;
                },
            }
        }
    }
}

// #[tracing::instrument(level = "debug")]
async fn select_conn(
    incoming: &mut h3_quinn::Endpoint,
    tasks: &mut tokio::task::JoinSet<Result<H3ConnQuinn, crate::Error>>,
) -> SelectOutputConn {
    tracing::debug!("select_conn");

    let incoming_stream_future = async {
        tracing::debug!("endpoint waiting accept");
        match incoming.accept().await {
            Some(i) => {
                tracing::debug!("endpoint accept incoming conn");
                SelectOutputConn::NewIncoming(i)
            }
            None => SelectOutputConn::Done, // shutdown.
        }
    };
    if tasks.is_empty() {
        tracing::debug!("endpoint wait for new incoming");
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

enum SelectOutputConn {
    NewIncoming(h3_quinn::quinn::Incoming),
    NewConn(H3ConnQuinn),
    ConnErr(crate::Error),
    Done,
}
