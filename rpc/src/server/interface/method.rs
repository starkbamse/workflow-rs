//! Module containing RPC [`Method`] closure wrappers
use crate::imports::*;

/// Base trait representing an RPC method, used to retain
/// method structures in an [`Interface`](super::Interface)
/// map without generics.
#[async_trait]
pub(crate) trait MethodTrait<ConnectionContext, ServerContext>:
    Send + Sync + 'static
{
    async fn call_with_borsh(
        &self,
        connection_ctx: Arc<ConnectionContext>,
        server_ctx: Arc<ServerContext>,
        data: &[u8],
    ) -> ServerResult<Vec<u8>>;
    async fn call_with_serde_json(
        &self,
        connection_ctx: Arc<ConnectionContext>,
        server_ctx: Arc<ServerContext>,
        value: Value,
    ) -> ServerResult<Value>;
}

/// RPC method function type
pub type MethodFn<ConnectionContext, ServerContext, Req, Resp> = Arc<
    Box<
        dyn Send
            + Sync
            + Fn(Arc<ConnectionContext>, Arc<ServerContext>, Req) -> MethodFnReturn<Resp>
            + 'static,
    >,
>;

/// RPC method function return type
pub type MethodFnReturn<T> =
    Pin<Box<(dyn Send + Sync + 'static + Future<Output = ServerResult<T>>)>>;

/// RPC method wrapper. Contains the method closure function.
pub struct Method<ConnectionContext, ServerContext, Req, Resp>
where
    ServerContext: Send + Sync + 'static,
    Req: MsgT,
    Resp: MsgT,
{
    method: MethodFn<ConnectionContext, ServerContext, Req, Resp>,
}

impl<ConnectionContext, ServerContext, Req, Resp>
    Method<ConnectionContext, ServerContext, Req, Resp>
where
    ServerContext: Send + Sync + 'static,
    Req: MsgT,
    Resp: MsgT,
{
    pub fn new<FN>(method_fn: FN) -> Method<ConnectionContext, ServerContext, Req, Resp>
    where
        FN: Send
            + Sync
            + Fn(Arc<ConnectionContext>, Arc<ServerContext>, Req) -> MethodFnReturn<Resp>
            + 'static,
    {
        Method {
            method: Arc::new(Box::new(method_fn)),
        }
    }
}

#[async_trait]
impl<ConnectionContext, ServerContext, Req, Resp> MethodTrait<ConnectionContext, ServerContext>
    for Method<ConnectionContext, ServerContext, Req, Resp>
where
    ConnectionContext: Send + Sync + 'static,
    ServerContext: Send + Sync + 'static,
    Req: MsgT,
    Resp: MsgT,
{
    async fn call_with_borsh(
        &self,
        connection_ctx: Arc<ConnectionContext>,
        method_ctx: Arc<ServerContext>,
        data: &[u8],
    ) -> ServerResult<Vec<u8>> {
        let req = Req::try_from_slice(data)?;
        let resp = (self.method)(connection_ctx, method_ctx, req).await;
        let vec = <ServerResult<Resp> as BorshSerialize>::try_to_vec(&resp)?;
        Ok(vec)
    }

    async fn call_with_serde_json(
        &self,
        connection_ctx: Arc<ConnectionContext>,
        method_ctx: Arc<ServerContext>,
        value: Value,
    ) -> ServerResult<Value> {
        let req: Req = serde_json::from_value(value).map_err(|_| ServerError::ReqDeserialize)?;
        let resp = (self.method)(connection_ctx, method_ctx, req).await?;
        Ok(serde_json::to_value(resp).map_err(|_| ServerError::RespSerialize)?)
    }
}