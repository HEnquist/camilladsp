// CamillaDSP - A flexible tool for processing audio
// Copyright (C) 2026 Henrik Enquist
//
// This file is part of CamillaDSP.
//
// CamillaDSP is free software; you can redistribute it and/or modify it
// under the terms of either:
//
// a) the GNU General Public License version 3,
//    or
// b) the Mozilla Public License Version 2.0.
//
// You should have received copies of the GNU General Public License and the
// Mozilla Public License along with this program. If not, see
// <https://www.gnu.org/licenses/> and <https://www.mozilla.org/MPL/2.0/>.

//! Prints the OpenAPI schemas of the websocket protocol types as JSON.
//! scripts/gen_ws_docs.py generates websocket.md from this output.

use camilladsp_schema::protocol::{WsCommand, WsReply, WsResult};
use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(components(schemas(WsCommand, WsReply, WsResult)))]
struct WsApi;

fn main() {
    println!("{}", WsApi::openapi().to_pretty_json().unwrap());
}
