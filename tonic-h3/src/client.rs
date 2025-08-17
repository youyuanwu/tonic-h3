use h3_util::client::H3Connection;

pub type H3Channel<C> = H3Connection<C, tonic::body::Body>;
