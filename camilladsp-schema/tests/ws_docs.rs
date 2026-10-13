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

//! Generates websocket.md from the OpenAPI schemas of the protocol types, and checks that the
//! file in the repository is up to date.
//!
//! The schemas are derived by utoipa from the Rust types and their serde attributes, so all
//! names are the wire names. The sections follow `WS_COMMAND_GROUPS`, and the text around them
//! is in `ws_template.md.j2`. Regenerate the file with
//!
//! ```sh
//! UPDATE_DOCS=1 cargo test -p camilladsp-schema --features utoipa --test ws_docs
//! ```

use camilladsp_schema::protocol::{WS_COMMAND_GROUPS, WsCommand, WsReply, WsResult};
use minijinja::syntax::SyntaxConfig;
use minijinja::{AutoEscape, Environment};
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::collections::HashSet;
use std::path::PathBuf;
use utoipa::OpenApi;

const TEMPLATE: &str = include_str!("ws_template.md.j2");
const REGENERATE: &str =
    "UPDATE_DOCS=1 cargo test -p camilladsp-schema --features utoipa --test ws_docs";

/// The message enums themselves, documented as commands, replies and errors rather than as types.
const MESSAGE_TYPES: [&str; 3] = ["WsCommand", "WsReply", "WsResult"];

#[derive(OpenApi)]
#[openapi(components(schemas(WsCommand, WsReply, WsResult)))]
struct WsApi;

#[derive(Serialize)]
struct Field {
    name: String,
    #[serde(rename = "type")]
    type_md: String,
    optional: bool,
    docs: String,
}

#[derive(Serialize)]
struct Command {
    name: String,
    docs: String,
    args: Vec<Field>,
    returns: String,
}

#[derive(Serialize)]
struct Group {
    name: String,
    description: String,
    commands: Vec<Command>,
}

#[derive(Serialize)]
struct OtherReply {
    name: String,
    docs: String,
    fields: Vec<Field>,
}

#[derive(Serialize)]
struct TypeOption {
    name: String,
    docs: String,
}

#[derive(Serialize)]
struct TypeEntry {
    name: String,
    docs: String,
    options: Vec<TypeOption>,
    fields: Vec<Field>,
    plain_values: String,
}

#[derive(Serialize)]
struct ErrorEntry {
    name: String,
    docs: String,
}

/// The template context.
#[derive(Serialize)]
struct Document {
    groups: Vec<Group>,
    other_replies: Vec<OtherReply>,
    types: Vec<TypeEntry>,
    errors: Vec<ErrorEntry>,
}

// ── Schema helpers ──────────────────────────────────────────────────────────

fn ref_name(schema: &Value) -> Option<&str> {
    schema.get("$ref")?.as_str()?.rsplit('/').next()
}

/// The GitHub heading anchor for a heading that is just the name in backticks.
fn anchor(name: &str) -> String {
    name.to_lowercase()
}

/// For a `oneOf: [X, {type: null}]` schema, returns X.
fn nullable_inner(schema: &Value) -> Option<&Value> {
    let null = json!({"type": "null"});
    let options = schema.get("oneOf")?.as_array()?;
    if options.len() == 2 && options.contains(&null) {
        options.iter().find(|o| **o != null)
    } else {
        None
    }
}

/// The description of a schema, also looking inside a nullable wrapper.
fn description_of(schema: &Value) -> &str {
    schema
        .get("description")
        .or_else(|| nullable_inner(schema)?.get("description"))
        .and_then(Value::as_str)
        .unwrap_or("")
}

fn primitive_md(type_name: &str, schema: &Value) -> String {
    match (type_name, schema.get("minimum")) {
        ("integer", Some(minimum)) => format!("`integer (≥ {minimum})`"),
        _ => format!("`{type_name}`"),
    }
}

/// Renders a schema as a Markdown type expression, linking named types to the Types section.
fn type_md(schema: &Value) -> String {
    if let Some(name) = ref_name(schema) {
        return format!("[`{name}`](#{})", anchor(name));
    }
    if let Some(inner) = nullable_inner(schema) {
        return format!("{} or `null`", type_md(inner));
    }
    match schema.get("type") {
        Some(Value::Array(types)) => {
            let rendered = types
                .iter()
                .filter(|t| *t != "null")
                .map(|t| {
                    let mut single = schema.clone();
                    single["type"] = t.clone();
                    type_md(&single)
                })
                .collect::<Vec<_>>()
                .join(" or ");
            if types.iter().any(|t| t == "null") {
                format!("{rendered} or `null`")
            } else {
                rendered
            }
        }
        Some(Value::String(type_name)) if type_name == "array" => {
            if let Some(items) = schema.get("prefixItems").and_then(Value::as_array) {
                let parts = items.iter().map(type_md).collect::<Vec<_>>().join(", ");
                format!("array \\[{parts}\\]")
            } else {
                let items = type_md(schema.get("items").unwrap_or(&Value::Null));
                if items.contains(" or ") {
                    format!("array of ({items})")
                } else {
                    format!("array of {items}")
                }
            }
        }
        Some(Value::String(type_name)) => primitive_md(type_name, schema),
        _ => {
            let only_description = match schema {
                Value::Bool(true) | Value::Null => true,
                Value::Object(map) => map.keys().all(|k| k == "description"),
                _ => false,
            };
            assert!(only_description, "unhandled schema shape {schema}");
            "any JSON value".to_string()
        }
    }
}

/// Removes the Markdown from a rendered type, for use inside a code span.
fn plain_type(md: &str) -> String {
    let mut out = String::new();
    let mut rest = md;
    while let Some(c) = rest.chars().next() {
        if let Some(link) = rest.strip_prefix("](#") {
            rest = link.split_once(')').map_or("", |(_, tail)| tail);
        } else {
            if !matches!(c, '`' | '\\' | '[') {
                out.push(c);
            }
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

/// Collects the names of all named types a schema refers to, in order of appearance.
fn referenced_types(schema: &Value, found: &mut Vec<String>) {
    match schema {
        Value::Object(map) => {
            if let Some(name) = ref_name(schema)
                && !found.iter().any(|f| f == name)
            {
                found.push(name.to_string());
            }
            for value in map.values() {
                referenced_types(value, found);
            }
        }
        Value::Array(items) => {
            for value in items {
                referenced_types(value, found);
            }
        }
        _ => {}
    }
}

/// The tag value of one variant of an internally tagged enum.
fn tag_value<'a>(schema: &'a Value, tag: &str) -> Option<&'a str> {
    let parts = match schema.get("allOf").and_then(Value::as_array) {
        Some(parts) => parts.iter().collect(),
        None => vec![schema],
    };
    parts.into_iter().find_map(|part| {
        part.get("properties")?
            .get(tag)?
            .get("enum")?
            .get(0)?
            .as_str()
    })
}

/// Merges the properties of a variant of an internally tagged enum, leaving out the tag and the
/// flattened WsResult. Returns the properties in declaration order and the required names.
fn variant_properties(schema: &Value, tag: &str) -> (Map<String, Value>, Vec<String>) {
    let mut properties = Map::new();
    let mut required = Vec::new();
    let parts = match schema.get("allOf").and_then(Value::as_array) {
        Some(parts) => parts.iter().collect(),
        None => vec![schema],
    };
    for part in parts {
        if ref_name(part).is_some() {
            continue;
        }
        if let Some(props) = part.get("properties").and_then(Value::as_object) {
            for (name, prop) in props {
                if name != tag {
                    properties.insert(name.clone(), prop.clone());
                }
            }
        }
        if let Some(req) = part.get("required").and_then(Value::as_array) {
            required.extend(
                req.iter()
                    .filter_map(Value::as_str)
                    .filter(|r| *r != tag)
                    .map(String::from),
            );
        }
    }
    (properties, required)
}

// ── Doc text ────────────────────────────────────────────────────────────────

/// Which intra-doc link targets have a heading in the document.
struct Links {
    commands: HashSet<String>,
    other_replies: HashSet<String>,
    errors: HashSet<String>,
}

impl Links {
    /// Turns rustdoc intra-doc links into document links, and strips the rest of the link
    /// syntax. `[`WsReply::X`]` links to the heading of X under "Other replies", `[`WsResult::X`]`
    /// to its heading under "Error responses" and `[`WsCommand::X`]` to the command. Any other
    /// link becomes a plain code span.
    fn convert(&self, text: &str) -> String {
        let mut out = String::new();
        let mut rest = text;
        while let Some(start) = rest.find("[`") {
            out.push_str(&rest[..start]);
            let after = &rest[start + 2..];
            let Some(end) = after.find("`]") else {
                out.push_str(&rest[start..]);
                rest = "";
                break;
            };
            let code = &after[..end];
            rest = &after[end + 2..];
            if let Some(target) = rest.strip_prefix('(')
                && let Some((_, tail)) = target.split_once(')')
            {
                rest = tail;
            }
            let link_name = code
                .strip_prefix("WsReply::")
                .filter(|n| self.other_replies.contains(*n))
                .or_else(|| {
                    code.strip_prefix("WsResult::")
                        .filter(|n| self.errors.contains(*n))
                })
                .or_else(|| {
                    code.strip_prefix("WsCommand::")
                        .filter(|n| self.commands.contains(*n))
                });
            match link_name {
                Some(name) => out.push_str(&format!("[`{name}`](#{})", anchor(name))),
                None => out.push_str(&format!("`{code}`")),
            }
        }
        out.push_str(rest);
        out.trim().to_string()
    }

    /// Joins a multi-paragraph doc into one line, for use in a list entry.
    fn one_line(&self, text: &str) -> String {
        self.convert(text)
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Group descriptions come from doc comments, where rustdoc keeps a leading space on each line.
fn trim_lines(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

// ── Template data ───────────────────────────────────────────────────────────

fn field_entries(
    links: &Links,
    properties: &Map<String, Value>,
    required: &[String],
    skip: &str,
) -> Vec<Field> {
    properties
        .iter()
        .filter(|(name, _)| *name != skip)
        .map(|(name, prop)| Field {
            name: name.clone(),
            type_md: type_md(prop),
            optional: !required.contains(name),
            docs: links.one_line(description_of(prop)),
        })
        .collect()
}

fn command_entry(links: &Links, schema: &Value, reply: Option<&Value>) -> Command {
    let name = tag_value(schema, "command").expect("command without a name");
    let (properties, required) = variant_properties(schema, "command");
    let returns = reply
        .and_then(|r| variant_properties(r, "reply").0.get("value").cloned())
        .map(|value| {
            let value_docs = links.one_line(description_of(&value));
            if value_docs.is_empty() {
                type_md(&value)
            } else {
                format!("({}): {value_docs}", type_md(&value))
            }
        })
        .unwrap_or_default();
    Command {
        name: name.to_string(),
        docs: links.convert(schema["description"].as_str().unwrap_or("")),
        args: field_entries(links, &properties, &required, "command"),
        returns,
    }
}

fn other_reply_entry(links: &Links, name: &str, schema: &Value) -> OtherReply {
    let (properties, required) = variant_properties(schema, "reply");
    OtherReply {
        name: name.to_string(),
        docs: links.convert(schema["description"].as_str().unwrap_or("")),
        fields: field_entries(links, &properties, &required, "reply"),
    }
}

/// Describes one named type for the Types section.
fn describe_type(links: &Links, name: &str, schema: &Value) -> TypeEntry {
    let mut entry = TypeEntry {
        name: name.to_string(),
        docs: links.convert(schema["description"].as_str().unwrap_or("")),
        options: Vec::new(),
        fields: Vec::new(),
        plain_values: String::new(),
    };
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        // A plain enum has no per-value docs, so its values go on one line.
        entry.plain_values = values
            .iter()
            .map(|v| format!("`{v}`"))
            .collect::<Vec<_>>()
            .join(", ");
    } else if let Some(variants) = schema.get("oneOf").and_then(Value::as_array) {
        for variant in variants {
            let docs = links.one_line(variant["description"].as_str().unwrap_or(""));
            let option_name = if let Some(value) = variant.get("enum").and_then(|e| e.get(0)) {
                value.to_string()
            } else {
                // An externally tagged variant with data, an object with a single key.
                let properties = variant["properties"]
                    .as_object()
                    .expect("variant without properties");
                assert_eq!(properties.len(), 1, "unexpected variant shape in {name}");
                let (key, prop) = properties.iter().next().unwrap();
                format!("{{\"{key}\": {}}}", plain_type(&type_md(prop)))
            };
            entry.options.push(TypeOption {
                name: option_name,
                docs,
            });
        }
    } else {
        let properties = schema
            .get("properties")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let required: Vec<String> = schema
            .get("required")
            .and_then(Value::as_array)
            .map(|r| {
                r.iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
        entry.fields = field_entries(links, &properties, &required, "");
    }
    entry
}

/// Renders websocket.md. Panics if the documentation is incomplete.
fn render() -> String {
    let api = serde_json::to_value(WsApi::openapi()).unwrap();
    let schemas = &api["components"]["schemas"];

    let command_schemas: Vec<&Value> = schemas["WsCommand"]["oneOf"]
        .as_array()
        .unwrap()
        .iter()
        .collect();
    let reply_schemas: Vec<&Value> = schemas["WsReply"]["oneOf"]
        .as_array()
        .unwrap()
        .iter()
        .collect();
    let replies: Vec<(&str, &Value)> = reply_schemas
        .iter()
        .map(|r| (tag_value(r, "reply").expect("reply without a name"), *r))
        .collect();
    let command_names: HashSet<&str> = WS_COMMAND_GROUPS
        .iter()
        .flat_map(|(_, _, commands)| commands.iter().copied())
        .collect();

    // Errors are the WsResult variants other than Ok.
    let error_schemas: Vec<(&str, &Value)> = schemas["WsResult"]["oneOf"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (tag_value(e, "result").expect("result without a name"), e))
        .filter(|(name, _)| *name != "Ok")
        .collect();

    // Replies that do not answer a command: pushed events, and the reply to an invalid message.
    let other_reply_schemas: Vec<(&str, &Value)> = replies
        .iter()
        .copied()
        .filter(|(name, _)| !command_names.contains(name))
        .collect();

    let links = Links {
        commands: command_names.iter().map(|n| n.to_string()).collect(),
        other_replies: other_reply_schemas
            .iter()
            .map(|(n, _)| n.to_string())
            .collect(),
        errors: error_schemas.iter().map(|(n, _)| n.to_string()).collect(),
    };

    let mut problems: Vec<String> = Vec::new();

    // The commands of each group, in the order of the groups.
    let mut groups = Vec::new();
    for (group_name, description, names) in WS_COMMAND_GROUPS {
        let mut commands = Vec::new();
        for name in *names {
            let schema = command_schemas
                .iter()
                .find(|c| tag_value(c, "command") == Some(name))
                .unwrap_or_else(|| panic!("command {name} is missing in the schema"));
            let reply = replies.iter().find(|(n, _)| n == name).map(|(_, r)| *r);
            if reply.is_none() {
                problems.push(format!("command {name} has no reply"));
            }
            let command = command_entry(&links, schema, reply);
            if command.docs.is_empty() {
                problems.push(format!("command {name} has no description"));
            }
            commands.push(command);
        }
        let description = trim_lines(description);
        if description.is_empty() {
            problems.push(format!("group {group_name} has no description"));
        }
        groups.push(Group {
            name: group_name.to_string(),
            description,
            commands,
        });
    }

    let other_replies: Vec<OtherReply> = other_reply_schemas
        .iter()
        .map(|(name, schema)| other_reply_entry(&links, name, schema))
        .collect();
    for reply in &other_replies {
        if reply.docs.is_empty() {
            problems.push(format!("reply {} has no description", reply.name));
        }
    }

    let errors: Vec<ErrorEntry> = error_schemas
        .iter()
        .map(|(name, schema)| ErrorEntry {
            name: name.to_string(),
            docs: links.convert(schema["description"].as_str().unwrap_or("")),
        })
        .collect();
    for error in &errors {
        if error.docs.is_empty() {
            problems.push(format!("error {} has no description", error.name));
        }
    }

    // Every named type the messages use, in order of first appearance in the command groups,
    // then the types those refer to.
    let mut type_names: Vec<String> = Vec::new();
    for group in &groups {
        for command in &group.commands {
            for schema in command_schemas
                .iter()
                .filter(|c| tag_value(c, "command") == Some(&command.name))
            {
                referenced_types(schema, &mut type_names);
            }
            for (_, reply) in replies.iter().filter(|(n, _)| *n == command.name) {
                referenced_types(reply, &mut type_names);
            }
        }
    }
    for (_, reply) in &other_reply_schemas {
        referenced_types(reply, &mut type_names);
    }
    let mut i = 0;
    while i < type_names.len() {
        let schema = schemas
            .get(&type_names[i])
            .unwrap_or_else(|| panic!("type {} has no schema", type_names[i]));
        referenced_types(schema, &mut type_names);
        i += 1;
    }
    let types: Vec<TypeEntry> = type_names
        .iter()
        .filter(|n| !MESSAGE_TYPES.contains(&n.as_str()))
        .map(|n| describe_type(&links, n, &schemas[n]))
        .collect();
    for t in &types {
        if t.docs.is_empty() {
            problems.push(format!("type {} has no description", t.name));
        }
    }

    // The type headings and the command headings share one anchor namespace.
    for t in &types {
        if command_names
            .iter()
            .any(|c| c.eq_ignore_ascii_case(&t.name))
        {
            problems.push(format!("type {} clashes with a command name", t.name));
        }
    }

    assert!(
        problems.is_empty(),
        "websocket docs are incomplete:\n{}",
        problems.join("\n")
    );

    let mut env = Environment::new();
    env.set_auto_escape_callback(|_| AutoEscape::None);
    env.set_syntax(
        SyntaxConfig::builder()
            .trim_blocks(true)
            .lstrip_blocks(true)
            .keep_trailing_newline(true)
            .build()
            .unwrap(),
    );
    env.add_template("websocket.md", TEMPLATE).unwrap();
    let document = Document {
        groups,
        other_replies,
        types,
        errors,
    };
    env.get_template("websocket.md")
        .unwrap()
        .render(minijinja::value::Serde(document))
        .unwrap()
}

#[test]
fn websocket_md_is_up_to_date() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../websocket.md");
    let generated = render();
    if std::env::var_os("UPDATE_DOCS").is_some_and(|v| !v.is_empty()) {
        std::fs::write(&path, &generated).unwrap();
        return;
    }
    // Normalize CRLF, for Windows checkouts.
    let current = std::fs::read_to_string(&path)
        .unwrap()
        .replace("\r\n", "\n");
    assert!(
        current == generated,
        "websocket.md is out of date. Regenerate it with:\n\n    {REGENERATE}\n"
    );
}
