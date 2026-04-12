use h3_util::client::H3Channel as Channel;

pub type H3Channel<C> = Channel<C, tonic::body::Body>;
