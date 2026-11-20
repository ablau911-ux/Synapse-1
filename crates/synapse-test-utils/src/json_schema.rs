//! Client-side JSON-Schema checks that mirror strict production MCP clients
//! (D1 client-parity, #548/#549).
//!
//! Strict clients do two things the raw JSON-RPC test client used to skip:
//! 1. they refuse a `tools/list` whose tool schemas carry a bare boolean in a
//!    subschema position (Zod `Invalid input` on `properties.<field>`), and
//! 2. they validate call arguments against `inputSchema` (and
//!    `structuredContent` against `outputSchema`) before trusting a call.
//!
//! No JSON-Schema crate is a dev-dependency of the workspace, so this is a
//! self-contained validator for the draft 2020-12 keyword subset that
//! `schemars` emits for Synapse tools. Unknown keywords are ignored (permissive),
//! exactly like a validator that does not know an annotation.

use serde_json::{Map, Value};

const MAX_DEPTH: usize = 64;

/// Keywords whose value is a map of subschemas.
const SCHEMA_MAP_KEYWORDS: &[&str] = &[
    "$defs",
    "definitions",
    "dependentSchemas",
    "patternProperties",
    "properties",
];
/// Keywords whose value is an array of subschemas.
const SCHEMA_ARRAY_KEYWORDS: &[&str] = &["oneOf", "anyOf", "allOf", "prefixItems"];
/// Keywords whose value is a single subschema.
const SCHEMA_VALUE_KEYWORDS: &[&str] = &[
    "contains",
    "else",
    "if",
    "items",
    "not",
    "propertyNames",
    "then",
];

/// Validates JSON instances against one root schema (resolving local `$ref`s).
pub struct SchemaValidator<'a> {
    root: &'a Value,
}

impl<'a> SchemaValidator<'a> {
    #[must_use]
    pub const fn new(root: &'a Value) -> Self {
        Self { root }
    }

    /// `Ok(())` when `instance` satisfies the schema; otherwise every violation,
    /// joined with `; `, each prefixed by its JSON path.
    pub fn validate(&self, instance: &Value) -> Result<(), String> {
        let mut errors = Vec::new();
        self.check(self.root, instance, "$", 0, &mut errors);
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }

    fn is_valid(&self, schema: &Value, instance: &Value, depth: usize) -> bool {
        let mut errors = Vec::new();
        self.check(schema, instance, "$", depth, &mut errors);
        errors.is_empty()
    }

    fn check(
        &self,
        schema: &Value,
        instance: &Value,
        path: &str,
        depth: usize,
        errors: &mut Vec<String>,
    ) {
        if depth > MAX_DEPTH {
            errors.push(format!("{path}: schema nesting exceeds {MAX_DEPTH}"));
            return;
        }
        let schema = match schema {
            Value::Bool(true) => return,
            Value::Bool(false) => {
                errors.push(format!("{path}: schema `false` admits no value"));
                return;
            }
            Value::Object(schema) => schema,
            _ => return,
        };
        if let Some(Value::String(reference)) = schema.get("$ref") {
            match self.resolve(reference) {
                Some(target) => self.check(target, instance, path, depth + 1, errors),
                None => errors.push(format!("{path}: unresolvable $ref `{reference}`")),
            }
        }
        check_type(schema, instance, path, errors);
        check_enum_const(schema, instance, path, errors);
        check_number(schema, instance, path, errors);
        check_string(schema, instance, path, errors);
        self.check_array(schema, instance, path, depth, errors);
        self.check_object(schema, instance, path, depth, errors);
        self.check_combinators(schema, instance, path, depth, errors);
    }

    fn resolve(&self, reference: &str) -> Option<&'a Value> {
        let pointer = reference.strip_prefix('#')?;
        if pointer.is_empty() {
            return Some(self.root);
        }
        self.root.pointer(pointer)
    }

    fn check_array(
        &self,
        schema: &Map<String, Value>,
        instance: &Value,
        path: &str,
        depth: usize,
        errors: &mut Vec<String>,
    ) {
        let Value::Array(items) = instance else {
            return;
        };
        if let Some(min) = schema.get("minItems").and_then(Value::as_u64)
            && (items.len() as u64) < min
        {
            errors.push(format!("{path}: {} items < minItems {min}", items.len()));
        }
        if let Some(max) = schema.get("maxItems").and_then(Value::as_u64)
            && (items.len() as u64) > max
        {
            errors.push(format!("{path}: {} items > maxItems {max}", items.len()));
        }
        if schema.get("uniqueItems") == Some(&Value::Bool(true)) {
            for (index, item) in items.iter().enumerate() {
                if items[..index].contains(item) {
                    errors.push(format!("{path}[{index}]: duplicate item (uniqueItems)"));
                }
            }
        }
        let prefix_len = match schema.get("prefixItems") {
            Some(Value::Array(prefix)) => {
                for (index, (sub, item)) in prefix.iter().zip(items).enumerate() {
                    self.check(sub, item, &format!("{path}[{index}]"), depth + 1, errors);
                }
                prefix.len()
            }
            _ => 0,
        };
        match schema.get("items") {
            Some(Value::Array(tuple)) => {
                for (index, (sub, item)) in tuple.iter().zip(items).enumerate() {
                    self.check(sub, item, &format!("{path}[{index}]"), depth + 1, errors);
                }
            }
            Some(sub) => {
                for (index, item) in items.iter().enumerate().skip(prefix_len) {
                    self.check(sub, item, &format!("{path}[{index}]"), depth + 1, errors);
                }
            }
            None => {}
        }
    }

    fn check_object(
        &self,
        schema: &Map<String, Value>,
        instance: &Value,
        path: &str,
        depth: usize,
        errors: &mut Vec<String>,
    ) {
        let Value::Object(fields) = instance else {
            return;
        };
        if let Some(Value::Array(required)) = schema.get("required") {
            for name in required.iter().filter_map(Value::as_str) {
                if !fields.contains_key(name) {
                    errors.push(format!("{path}: missing required field `{name}`"));
                }
            }
        }
        let properties = schema.get("properties").and_then(Value::as_object);
        let patterns = schema.get("patternProperties").and_then(Value::as_object);
        for (name, value) in fields {
            let child = format!("{path}.{name}");
            let mut matched = false;
            if let Some(sub) = properties.and_then(|props| props.get(name)) {
                matched = true;
                self.check(sub, value, &child, depth + 1, errors);
            }
            for (pattern, sub) in patterns.into_iter().flatten() {
                if regex::Regex::new(pattern).is_ok_and(|re| re.is_match(name)) {
                    matched = true;
                    self.check(sub, value, &child, depth + 1, errors);
                }
            }
            if matched {
                continue;
            }
            match schema.get("additionalProperties") {
                Some(Value::Bool(false)) => errors.push(format!(
                    "{path}: unknown field `{name}` (additionalProperties: false)"
                )),
                Some(sub @ Value::Object(_)) => {
                    self.check(sub, value, &child, depth + 1, errors);
                }
                _ => {}
            }
        }
        if let Some(names) = schema.get("propertyNames") {
            for name in fields.keys() {
                let key = Value::String(name.clone());
                self.check(names, &key, &format!("{path}.<{name}>"), depth + 1, errors);
            }
        }
        if let Some(min) = schema.get("minProperties").and_then(Value::as_u64)
            && (fields.len() as u64) < min
        {
            errors.push(format!(
                "{path}: {} fields < minProperties {min}",
                fields.len()
            ));
        }
        if let Some(max) = schema.get("maxProperties").and_then(Value::as_u64)
            && (fields.len() as u64) > max
        {
            errors.push(format!(
                "{path}: {} fields > maxProperties {max}",
                fields.len()
            ));
        }
    }

    fn check_combinators(
        &self,
        schema: &Map<String, Value>,
        instance: &Value,
        path: &str,
        depth: usize,
        errors: &mut Vec<String>,
    ) {
        if let Some(Value::Array(all)) = schema.get("allOf") {
            for sub in all {
                self.check(sub, instance, path, depth + 1, errors);
            }
        }
        if let Some(Value::Array(any)) = schema.get("anyOf")
            && !any
                .iter()
                .any(|sub| self.is_valid(sub, instance, depth + 1))
        {
            errors.push(format!(
                "{path}: matches none of {} anyOf branches",
                any.len()
            ));
        }
        if let Some(Value::Array(one)) = schema.get("oneOf") {
            let matches = one
                .iter()
                .filter(|sub| self.is_valid(sub, instance, depth + 1))
                .count();
            if matches != 1 {
                errors.push(format!(
                    "{path}: matches {matches} of {} oneOf branches (exactly 1 required)",
                    one.len()
                ));
            }
        }
        if let Some(not) = schema.get("not")
            && self.is_valid(not, instance, depth + 1)
        {
            errors.push(format!("{path}: value matches a `not` schema"));
        }
        if let Some(condition) = schema.get("if") {
            let branch = if self.is_valid(condition, instance, depth + 1) {
                schema.get("then")
            } else {
                schema.get("else")
            };
            if let Some(branch) = branch {
                self.check(branch, instance, path, depth + 1, errors);
            }
        }
    }
}

fn type_matches(expected: &str, instance: &Value) -> bool {
    match expected {
        "null" => instance.is_null(),
        "boolean" => instance.is_boolean(),
        "object" => instance.is_object(),
        "array" => instance.is_array(),
        "string" => instance.is_string(),
        "number" => instance.is_number(),
        "integer" => {
            instance.is_i64()
                || instance.is_u64()
                || instance
                    .as_f64()
                    .is_some_and(|value| value.is_finite() && value.fract().abs() < f64::EPSILON)
        }
        _ => true,
    }
}

fn type_name(instance: &Value) -> &'static str {
    match instance {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn check_type(schema: &Map<String, Value>, instance: &Value, path: &str, errors: &mut Vec<String>) {
    let ok = match schema.get("type") {
        Some(Value::String(expected)) => type_matches(expected, instance),
        Some(Value::Array(options)) => options
            .iter()
            .filter_map(Value::as_str)
            .any(|expected| type_matches(expected, instance)),
        _ => true,
    };
    if !ok {
        errors.push(format!(
            "{path}: type {} does not match schema type {}",
            type_name(instance),
            schema
                .get("type")
                .map_or_else(String::new, Value::to_string)
        ));
    }
}

fn check_enum_const(
    schema: &Map<String, Value>,
    instance: &Value,
    path: &str,
    errors: &mut Vec<String>,
) {
    if let Some(Value::Array(options)) = schema.get("enum")
        && !options.contains(instance)
    {
        errors.push(format!("{path}: {instance} is not one of enum {options:?}"));
    }
    if let Some(expected) = schema.get("const")
        && expected != instance
    {
        errors.push(format!("{path}: {instance} != const {expected}"));
    }
}

fn check_number(
    schema: &Map<String, Value>,
    instance: &Value,
    path: &str,
    errors: &mut Vec<String>,
) {
    let Some(value) = instance.as_f64() else {
        return;
    };
    let bound = |key: &str| schema.get(key).and_then(Value::as_f64);
    if let Some(min) = bound("minimum")
        && value < min
    {
        errors.push(format!("{path}: {value} < minimum {min}"));
    }
    if let Some(max) = bound("maximum")
        && value > max
    {
        errors.push(format!("{path}: {value} > maximum {max}"));
    }
    if let Some(min) = bound("exclusiveMinimum")
        && value <= min
    {
        errors.push(format!("{path}: {value} <= exclusiveMinimum {min}"));
    }
    if let Some(max) = bound("exclusiveMaximum")
        && value >= max
    {
        errors.push(format!("{path}: {value} >= exclusiveMaximum {max}"));
    }
}

fn check_string(
    schema: &Map<String, Value>,
    instance: &Value,
    path: &str,
    errors: &mut Vec<String>,
) {
    let Some(text) = instance.as_str() else {
        return;
    };
    let length = text.chars().count() as u64;
    if let Some(min) = schema.get("minLength").and_then(Value::as_u64)
        && length < min
    {
        errors.push(format!("{path}: length {length} < minLength {min}"));
    }
    if let Some(max) = schema.get("maxLength").and_then(Value::as_u64)
        && length > max
    {
        errors.push(format!("{path}: length {length} > maxLength {max}"));
    }
    if let Some(Value::String(pattern)) = schema.get("pattern")
        && let Ok(re) = regex::Regex::new(pattern)
        && !re.is_match(text)
    {
        errors.push(format!(
            "{path}: `{text}` does not match pattern `{pattern}`"
        ));
    }
}

/// Schema-shape problems a strict MCP client rejects in `tools/list`: a bare
/// boolean in any subschema position (#548/#549). Booleans under
/// `additionalProperties` / `additionalItems` / `unevaluated*` stay legal.
#[must_use]
pub fn strict_client_schema_issues(schema: &Value) -> Vec<String> {
    let mut issues = Vec::new();
    lint_schema(schema, "$", &mut issues);
    issues
}

fn lint_schema(schema: &Value, path: &str, issues: &mut Vec<String>) {
    let Value::Object(map) = schema else {
        return;
    };
    for (key, child) in map {
        let child_path = format!("{path}.{key}");
        if SCHEMA_MAP_KEYWORDS.contains(&key.as_str()) {
            for (name, member) in child.as_object().into_iter().flatten() {
                lint_subschema(member, &format!("{child_path}.{name}"), issues);
            }
        } else if SCHEMA_ARRAY_KEYWORDS.contains(&key.as_str()) {
            for (index, element) in child.as_array().into_iter().flatten().enumerate() {
                lint_subschema(element, &format!("{child_path}[{index}]"), issues);
            }
        } else if SCHEMA_VALUE_KEYWORDS.contains(&key.as_str()) {
            match child {
                Value::Array(elements) => {
                    for (index, element) in elements.iter().enumerate() {
                        lint_subschema(element, &format!("{child_path}[{index}]"), issues);
                    }
                }
                _ => lint_subschema(child, &child_path, issues),
            }
        } else if matches!(key.as_str(), "additionalProperties" | "additionalItems")
            && child.is_object()
        {
            lint_schema(child, &child_path, issues);
        }
    }
}

fn lint_subschema(schema: &Value, path: &str, issues: &mut Vec<String>) {
    if schema.is_boolean() {
        issues.push(format!("{path}: bare boolean subschema `{schema}`"));
    } else {
        lint_schema(schema, path, issues);
    }
}
