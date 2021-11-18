use actix::dev::Stream;
use actix::{Actor, AsyncContext, Context, Handler, Message, StreamHandler};
use tracing::{info, warn};

#[derive(Debug)]
pub struct LoggerActor;

#[derive(Debug, Copy, Clone)]
pub enum IOType {
    Stdout,
    Stderr,
}

#[derive(Debug)]
pub struct ChildOutput {
    pub name: String,
    pub ty: IOType,
    pub data: String,
}

impl Actor for LoggerActor {
    type Context = Context<Self>;
}

impl StreamHandler<ChildOutput> for LoggerActor {
    fn handle(&mut self, item: ChildOutput, _ctx: &mut Self::Context) {
        match item.ty {
            IOType::Stdout => info!(?item.name, "{}", item.data),
            IOType::Stderr => warn!(?item.name, "{}", item.data),
        }
    }
}

#[derive(Debug, Message)]
#[rtype("()")]
pub struct RegisterStdio<S>(pub S);

impl<S> Handler<RegisterStdio<S>> for LoggerActor
where
    S: Stream<Item = ChildOutput> + 'static,
{
    type Result = ();

    fn handle(&mut self, msg: RegisterStdio<S>, ctx: &mut Self::Context) -> Self::Result {
        ctx.add_stream(msg.0);
    }
}
