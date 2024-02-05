use actix::dev::Stream;
use actix::{Actor, AsyncContext, Context, Handler, Message, StreamHandler};
use log::{info, log, warn, Level};

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
            IOType::Stdout => info!(target: &item.name, "{}", item.data),
            IOType::Stderr => warn!(target: &item.name, "{}", item.data),
        }
    }

    fn finished(&mut self, _: &mut Self::Context) {}
}

#[derive(Debug, Message)]
#[rtype(result = "()")]
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

#[derive(Debug, Message)]
#[rtype(result = "()")]
pub struct Custom {
    pub level: Level,
    pub name: String,
    pub data: String,
}

impl Custom {
    pub fn new(name: impl Into<String>, data: impl Into<String>) -> Self {
        Self::new_with_level(Level::Warn, name, data)
    }
    pub fn new_with_level(level: Level, name: impl Into<String>, data: impl Into<String>) -> Self {
        Self {
            level,
            name: name.into(),
            data: data.into(),
        }
    }
}

impl Handler<Custom> for LoggerActor {
    type Result = ();

    fn handle(&mut self, msg: Custom, _ctx: &mut Self::Context) -> Self::Result {
        log!(target: &msg.name, msg.level, "{}", msg.data);
    }
}
