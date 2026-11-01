// Generated from OpenSnitch 1.8.0 proto/ui.proto by tonic-build 0.12.3: the client side, only for tests.
pub mod ui_client {
    #![allow(
        unused_variables,
        dead_code,
        missing_docs,
        clippy::wildcard_imports,
        clippy::let_unit_value,
    )]
    use tonic::codegen::*;
    use tonic::codegen::http::Uri;
    #[derive(Debug, Clone)]
    pub struct UiClient<T> {
        inner: tonic::client::Grpc<T>,
    }
    impl<T> UiClient<T>
    where
        T: tonic::client::GrpcService<tonic::body::BoxBody>,
        T::Error: Into<StdError>,
        T::ResponseBody: Body<Data = Bytes> + std::marker::Send + 'static,
        <T::ResponseBody as Body>::Error: Into<StdError> + std::marker::Send,
    {
        pub fn new(inner: T) -> Self {
            let inner = tonic::client::Grpc::new(inner);
            Self { inner }
        }
        pub fn with_origin(inner: T, origin: Uri) -> Self {
            let inner = tonic::client::Grpc::with_origin(inner, origin);
            Self { inner }
        }
        pub fn with_interceptor<F>(
            inner: T,
            interceptor: F,
        ) -> UiClient<InterceptedService<T, F>>
        where
            F: tonic::service::Interceptor,
            T::ResponseBody: Default,
            T: tonic::codegen::Service<
                http::Request<tonic::body::BoxBody>,
                Response = http::Response<
                    <T as tonic::client::GrpcService<tonic::body::BoxBody>>::ResponseBody,
                >,
            >,
            <T as tonic::codegen::Service<
                http::Request<tonic::body::BoxBody>,
            >>::Error: Into<StdError> + std::marker::Send + std::marker::Sync,
        {
            UiClient::new(InterceptedService::new(inner, interceptor))
        }
        /// Compress requests with the given encoding.
        ///
        /// This requires the server to support it otherwise it might respond with an
        /// error.
        #[must_use]
        pub fn send_compressed(mut self, encoding: CompressionEncoding) -> Self {
            self.inner = self.inner.send_compressed(encoding);
            self
        }
        /// Enable decompressing responses.
        #[must_use]
        pub fn accept_compressed(mut self, encoding: CompressionEncoding) -> Self {
            self.inner = self.inner.accept_compressed(encoding);
            self
        }
        /// Limits the maximum size of a decoded message.
        ///
        /// Default: `4MB`
        #[must_use]
        pub fn max_decoding_message_size(mut self, limit: usize) -> Self {
            self.inner = self.inner.max_decoding_message_size(limit);
            self
        }
        /// Limits the maximum size of an encoded message.
        ///
        /// Default: `usize::MAX`
        #[must_use]
        pub fn max_encoding_message_size(mut self, limit: usize) -> Self {
            self.inner = self.inner.max_encoding_message_size(limit);
            self
        }
        pub async fn ping(
            &mut self,
            request: impl tonic::IntoRequest<super::PingRequest>,
        ) -> std::result::Result<tonic::Response<super::PingReply>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic::codec::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static("/protocol.UI/Ping");
            let mut req = request.into_request();
            req.extensions_mut().insert(GrpcMethod::new("protocol.UI", "Ping"));
            self.inner.unary(req, path, codec).await
        }
        pub async fn ask_rule(
            &mut self,
            request: impl tonic::IntoRequest<super::Connection>,
        ) -> std::result::Result<tonic::Response<super::Rule>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic::codec::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static("/protocol.UI/AskRule");
            let mut req = request.into_request();
            req.extensions_mut().insert(GrpcMethod::new("protocol.UI", "AskRule"));
            self.inner.unary(req, path, codec).await
        }
        pub async fn subscribe(
            &mut self,
            request: impl tonic::IntoRequest<super::ClientConfig>,
        ) -> std::result::Result<tonic::Response<super::ClientConfig>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic::codec::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static("/protocol.UI/Subscribe");
            let mut req = request.into_request();
            req.extensions_mut().insert(GrpcMethod::new("protocol.UI", "Subscribe"));
            self.inner.unary(req, path, codec).await
        }
        pub async fn notifications(
            &mut self,
            request: impl tonic::IntoStreamingRequest<Message = super::NotificationReply>,
        ) -> std::result::Result<
            tonic::Response<tonic::codec::Streaming<super::Notification>>,
            tonic::Status,
        > {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic::codec::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/protocol.UI/Notifications",
            );
            let mut req = request.into_streaming_request();
            req.extensions_mut().insert(GrpcMethod::new("protocol.UI", "Notifications"));
            self.inner.streaming(req, path, codec).await
        }
        pub async fn post_alert(
            &mut self,
            request: impl tonic::IntoRequest<super::Alert>,
        ) -> std::result::Result<tonic::Response<super::MsgResponse>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic::codec::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static("/protocol.UI/PostAlert");
            let mut req = request.into_request();
            req.extensions_mut().insert(GrpcMethod::new("protocol.UI", "PostAlert"));
            self.inner.unary(req, path, codec).await
        }
    }
}
