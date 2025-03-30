use h3_util::server::H3Acceptor;

/// Router for running tonic services.
///
pub struct H3Router(tonic::service::Routes);

impl H3Router {
    pub fn new(routes: tonic::service::Routes) -> Self {
        Self(routes)
    }
}

impl H3Router {
    /// Runs the service on acceptor until shutdown.
    pub async fn serve_with_shutdown<AC, F>(
        self,
        acceptor: AC,
        signal: F,
    ) -> Result<(), crate::Error>
    where
        AC: H3Acceptor,
        F: std::future::Future<Output = ()>,
    {
        let auxm_router = axum_h3::H3Router::from(self.0.prepare().into_axum_router());
        auxm_router.serve_with_shutdown(acceptor, signal).await
    }

    /// Runs all services on acceptor
    pub async fn serve<AC>(self, acceptor: AC) -> Result<(), crate::Error>
    where
        AC: H3Acceptor,
    {
        self.serve_with_shutdown(acceptor, async {
            // never returns
            futures::future::pending().await
        })
        .await
    }
}
