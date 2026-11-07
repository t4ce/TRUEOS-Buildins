//! URL parsing uses Hull's existing WHATWG-oriented `url` dependency. There is
//! no resource loader, network operation, navigation or persistent native store.
use alloc::{string::{String, ToString}, vec::Vec};
use super::{DomResult, failure, input};
use hull_quickjs::Engine;
use serde_json::{Value, json};
use url::{Url, form_urlencoded, quirks};
const MAX_BYTES: usize = 128 * 1024;
const MAX_PAIRS: usize = 4096;
fn text(args: &[Value], index: usize) -> DomResult<&str> {
    let s = input(args, index)?;
    if s.len() > MAX_BYTES {
        return Err(failure("RangeError", "URL string budget exceeded"));
    }
    Ok(s)
}
fn snapshot(u: &Url) -> DomResult<Value> {
    if u.as_str().len() > MAX_BYTES {
        return Err(failure("RangeError", "Serialized URL budget exceeded"));
    }
    Ok(
        json!({"href":quirks::href(u),"origin":quirks::origin(u),"protocol":quirks::protocol(u),"username":quirks::username(u),"password":quirks::password(u),"host":quirks::host(u),"hostname":quirks::hostname(u),"port":quirks::port(u),"pathname":quirks::pathname(u),"search":quirks::search(u),"hash":quirks::hash(u)}),
    )
}
pub(super) fn call(args: &[Value]) -> DomResult<Value> {
    match input(args, 1)? {
        "parse" => {
            let s = text(args, 2)?;
            let base = if args.get(3).is_none_or(Value::is_null) {
                None
            } else {
                match Url::parse(text(args, 3)?) {
                    Ok(u) => Some(u),
                    Err(_) => return Ok(Value::Null),
                }
            };
            match Url::options().base_url(base.as_ref()).parse(s) {
                Ok(u) => snapshot(&u),
                Err(_) => Ok(Value::Null),
            }
        }
        "set" => {
            let mut u =
                Url::parse(text(args, 2)?).map_err(|_| failure("TypeError", "Invalid URL"))?;
            let property = input(args, 3)?;
            let value = text(args, 4)?;
            match property {
                "href" => quirks::set_href(&mut u, value)
                    .map_err(|_| failure("TypeError", "Invalid URL"))?,
                "protocol" => {
                    let _ = quirks::set_protocol(&mut u, value);
                }
                "username" => {
                    let _ = quirks::set_username(&mut u, value);
                }
                "password" => {
                    let _ = quirks::set_password(&mut u, value);
                }
                "host" => {
                    let _ = quirks::set_host(&mut u, value);
                }
                "hostname" => {
                    let _ = quirks::set_hostname(&mut u, value);
                }
                "port" => {
                    let _ = quirks::set_port(&mut u, value);
                }
                "pathname" => quirks::set_pathname(&mut u, value),
                "search" => quirks::set_search(&mut u, value),
                "hash" => quirks::set_hash(&mut u, value),
                _ => return Err(failure("TypeError", "Unsupported URL attribute")),
            }
            snapshot(&u)
        }
        "queryParse" => {
            let s = text(args, 2)?;
            let mut pairs = Vec::new();
            for (k, v) in form_urlencoded::parse(s.strip_prefix('?').unwrap_or(s).as_bytes()) {
                if pairs.len() >= MAX_PAIRS {
                    return Err(failure(
                        "RangeError",
                        "URLSearchParams pair budget exceeded",
                    ));
                }
                pairs.push([k.into_owned(), v.into_owned()]);
            }
            Ok(json!(pairs))
        }
        "querySerialize" => {
            let pairs = args
                .get(2)
                .and_then(Value::as_array)
                .ok_or_else(|| failure("TypeError", "Expected query pairs"))?;
            if pairs.len() > MAX_PAIRS {
                return Err(failure(
                    "RangeError",
                    "URLSearchParams pair budget exceeded",
                ));
            }
            let mut output = form_urlencoded::Serializer::new(String::new());
            let mut bytes = 0usize;
            for pair in pairs {
                let pair = pair
                    .as_array()
                    .filter(|p| p.len() == 2)
                    .ok_or_else(|| failure("TypeError", "Expected a name/value pair"))?;
                let (k, v) = (text(pair, 0)?, text(pair, 1)?);
                bytes = bytes.saturating_add(k.len()).saturating_add(v.len());
                if bytes > MAX_BYTES {
                    return Err(failure(
                        "RangeError",
                        "URLSearchParams string budget exceeded",
                    ));
                }
                output.append_pair(k, v);
            }
            let output = output.finish();
            if output.len() > MAX_BYTES {
                return Err(failure("RangeError", "Serialized query budget exceeded"));
            }
            Ok(json!(output))
        }
        _ => Err(failure("TypeError", "Unsupported URL operation")),
    }
}

/// Console-only realms also get URL; only the URL router is installed there.
pub(super) fn install(engine: &mut Engine) -> Result<(), String> {
    install_with_timers(engine,None)
}
pub(super) fn install_with_timers(engine:&mut Engine,timers:Option<super::timers::SharedTimers>)->Result<(),String>{
    let enabled=timers.is_some();
    engine.bind_host_handler("__hullUrl", move |bytes| {
        let request: Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        let result = request
            .as_array()
            .ok_or_else(|| failure("TypeError", "Expected URL request"))
            .and_then(|args| {
                if input(args,0)?=="timer" {
                    return timers.as_ref().map(|state|super::timers::call(state,&args[1..])).ok_or_else(||failure("NotSupportedError","Timers absent"));
                }
                if input(args, 0)? != "url" {
                    return Err(failure("TypeError", "Unsupported host operation"));
                }
                call(args)
            });
        let response = match result {
            Ok(value) => json!({"ok":true,"value":value}),
            Err((name, message)) => json!({"ok":false,"name":name,"message":message}),
        };
        serde_json::to_vec(&response).map_err(|e| e.to_string())
    })?;
    let source=[r#"(()=>{'use strict';const native=globalThis.__hullUrl;delete globalThis.__hullUrl;const proto=Object.setPrototypeOf;function request(...args){proto(args,null);const r=native(args);if(!r.ok){if(r.name==='RangeError')throw new RangeError(r.message);throw new TypeError(r.message);}return r.value;} ("#,
        include_str!("javascript_url.js"),
        ")(request);",if enabled {"Object.defineProperty(globalThis,'__hullTimerSchedule',{value:args=>request('timer',args[0],args[1],args[2]??0),configurable:true});"}else{""},"})();"].concat();
    engine.eval(source.as_bytes(), "hull:url", false)
}
