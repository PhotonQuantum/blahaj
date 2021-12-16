//! Adopted from <https://github.com/svenstaro/proxyboi/blob/master/src/handler.rs>.
use actix_web::{web, HttpRequest, HttpResponse};
use awc::http::StatusCode;
use awc::Client;

use crate::error::HttpError;
use crate::proxy::ProxyConfig;

use super::forwarded_header::ForwardedHeader;

#[allow(clippy::future_not_send)]
pub async fn forward(
    incoming_request: HttpRequest,
    body: web::Bytes,
    config: web::Data<ProxyConfig>,
    client: web::Data<Client>,
) -> Result<HttpResponse, HttpError> {
    let upstream = if let Some(upstream) = config.get(incoming_request.uri()) {
        upstream
    } else {
        return Ok(HttpResponse::new(StatusCode::NOT_FOUND));
    };

    let conn_info = &incoming_request.connection_info().clone();
    let protocol = conn_info.scheme();
    let version = incoming_request.version();
    let host = conn_info.host();

    let peer = incoming_request
        .head()
        .peer_addr
        .map_or_else(|| "unknown".to_string(), |p| p.ip().to_string());

    let forwarded = incoming_request
        .headers()
        .get("forwarded")
        .map_or("", |x| x.to_str().unwrap_or(""));

    let forwarded_header = ForwardedHeader::from_info(
        &peer,
        &config.interface.to_string(),
        forwarded,
        host,
        protocol,
    );
    let via = incoming_request
        .headers()
        .get("via")
        .map(|x| x.to_str().unwrap_or(""))
        .map_or_else(
            || format!("{version:?} blahaj", version = version),
            |via| {
                format!(
                    "{previous_via}, {version:?} blahaj",
                    previous_via = via,
                    version = version
                )
            },
        );

    // The X-Forwarded-For header is much simpler to handle :)
    let x_forwarded_for = incoming_request
        .headers()
        .get("x-forwarded-for")
        .map(|x| x.to_str().unwrap_or(""));
    let x_forwarded_for_appended = if let Some(x_forwarded_for) = x_forwarded_for {
        format!("{}, {}", x_forwarded_for, peer)
    } else {
        peer
    };

    let upstream_req = client
        .request_from(&upstream.to_string(), incoming_request.head())
        .no_decompress()
        // https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Forwarded
        .insert_header(("forwarded", forwarded_header.to_string()))
        // https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/X-Forwarded-Proto
        .insert_header(("x-forwarded-proto", protocol))
        // https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/X-Forwarded-Host
        .insert_header(("x-forwarded-host", host))
        // https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/X-Forwarded-For
        .insert_header(("x-forwarded-for", x_forwarded_for_appended))
        // https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Via
        .insert_header(("via", via));

    let mut upstream_resp = upstream_req.send_body(body).await?;

    let mut outgoing_resp_builder = HttpResponse::build(upstream_resp.status());
    for (header_name, header_value) in upstream_resp
        .headers()
        .iter()
        // Remove `Connection` as per
        // https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Connection#Directives
        .filter(|(h, _)| *h != "connection" && *h != "transfer-encoding")
    {
        outgoing_resp_builder.insert_header((header_name, header_value.clone()));
    }

    let outgoing_resp = outgoing_resp_builder.body(upstream_resp.body().await?);

    Ok(outgoing_resp)
}
