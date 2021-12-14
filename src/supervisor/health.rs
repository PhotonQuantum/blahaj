use std::time::Duration;

use actix::{Actor, ActorContext, ActorFutureExt, Addr, AsyncContext, Context, WrapFuture};
use awc::http::Uri;
use awc::Client;
use log::Level;
use tokio::sync::oneshot::Sender;

use crate::error::HealthError;
use crate::logger::Custom;
use crate::supervisor::caretaker::{CaretakerActor, HealthState, SetHealthState};
use crate::LoggerActor;

pub struct HealthConfig {
    pub uri: Uri,
    pub interval: Duration,
    pub grace_period: Duration,
}

pub struct HealthActor {
    name: String,
    client: Client,
    config: HealthConfig,
    stop_tx: Option<Sender<()>>,
    unhealthy_count: usize,
    caretaker: Addr<CaretakerActor>,
    logger: Addr<LoggerActor>,
}

impl HealthActor {
    pub fn new(
        name: String,
        client: Client,
        config: HealthConfig,
        stop_tx: Sender<()>,
        caretaker: Addr<CaretakerActor>,
        logger: Addr<LoggerActor>,
    ) -> Self {
        Self {
            name,
            client,
            config,
            stop_tx: Some(stop_tx),
            unhealthy_count: 0,
            caretaker,
            logger,
        }
    }
}

impl Actor for HealthActor {
    type Context = Context<Self>;
    fn started(&mut self, ctx: &mut Self::Context) {
        ctx.run_interval(self.config.interval, move |act, ctx| {
            ctx.spawn(
                act.client
                    .get(&act.config.uri)
                    .send()
                    .into_actor(act)
                    .map(|res, _, _| {
                        res.map_err(HealthError::Send).and_then(|resp| {
                            if resp.status().is_success() {
                                Ok(())
                            } else {
                                Err(HealthError::Status(resp.status()))
                            }
                        })
                    })
                    .map(|res, act, ctx| {
                        if let Err(e) = res {
                            act.caretaker
                                .do_send(SetHealthState(HealthState::Unhealthy));
                            act.unhealthy_count += 1;

                            act.logger.do_send(Custom::new_with_level(
                                Level::Error,
                                act.name.clone(),
                                format!("Health check failed ({}): {}", act.unhealthy_count, e),
                            ));

                            if act.config.interval
                                * act.unhealthy_count.try_into().expect("doesn't overflow")
                                >= act.config.grace_period
                            {
                                act.stop_tx
                                    .take()
                                    .expect("can't be consumed twice")
                                    .send(())
                                    .expect("notify WaitAbortFut");
                                ctx.stop();
                            }
                        } else {
                            act.caretaker.do_send(SetHealthState(HealthState::Healthy));
                            act.unhealthy_count = 0;
                        };
                    }),
            );
        });
    }
}
