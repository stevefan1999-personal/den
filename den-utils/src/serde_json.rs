use ::serde_json::Value;
use derive_more::{From, Into};
use rquickjs::{Array, Ctx, Error, FromJs, IntoJs, Object, Result, Value as JsValue};

#[derive(From, Into, Clone, Eq, PartialEq, Hash)]
pub struct SerdeJsonValue(pub serde_json::Value);

impl<'js> FromJs<'js> for SerdeJsonValue {
    fn from_js(ctx: &Ctx<'js>, v: JsValue<'js>) -> Result<Self> {
        let value = match v.type_of() {
            rquickjs::Type::Null | rquickjs::Type::Uninitialized | rquickjs::Type::Undefined => {
                serde_json::Value::Null
            }
            rquickjs::Type::Bool => serde_json::json!(v.as_bool().unwrap_or_default()),
            rquickjs::Type::Int => serde_json::json!(v.as_int().unwrap_or_default()),
            rquickjs::Type::Float => serde_json::json!(v.as_float().unwrap_or_default()),
            rquickjs::Type::String => {
                serde_json::json!(
                    v.as_string()
                        .unwrap_or(&rquickjs::String::from_str(ctx.clone(), "")?)
                        .to_string()
                        .unwrap_or(String::from(""))
                )
            }
            rquickjs::Type::Array => {
                if let Some(arr) = v.as_array() {
                    let mut values = Vec::with_capacity(arr.len());
                    for entry in arr.clone().into_iter() {
                        values.push(SerdeJsonValue::from_js(ctx, entry?)?.0);
                    }
                    serde_json::Value::Array(values)
                } else {
                    serde_json::Value::Array(vec![])
                }
            }
            // rquickjs 0.12 reports a JS `Proxy` as its own type; it is still an object and
            // walking it lets its traps answer, which is what 0.8 did.
            rquickjs::Type::Object | rquickjs::Type::Proxy => {
                let mut map = serde_json::Map::<String, Value>::new();
                if let Some(obj) = v.as_object() {
                    for entry in obj.clone().into_iter() {
                        let (key, value) = entry?;
                        map.insert(
                            key.clone().to_string()?,
                            SerdeJsonValue::from_js(ctx, value)?.0,
                        );
                    }
                }
                serde_json::Value::Object(map)
            }
            // Functions, symbols and bigints have no JSON representation — the same values
            // `JSON.stringify` refuses. Report the conversion failure instead of panicking.
            other => return Err(Error::new_from_js(other.as_str(), "json value")),
        };
        Ok(SerdeJsonValue(value))
    }
}

impl<'js> IntoJs<'js> for SerdeJsonValue {
    fn into_js(self, ctx: &Ctx<'js>) -> Result<JsValue<'js>> {
        let ctx = ctx.clone();
        match self.0 {
            Value::Null => Ok(JsValue::new_null(ctx)),
            Value::Bool(x) => x.into_js(&ctx),
            Value::Number(x) if x.is_f64() => x.as_f64().unwrap().into_js(&ctx),
            Value::Number(x) if x.is_i64() => x.as_i64().unwrap().into_js(&ctx),
            Value::Number(x) if x.is_u64() => x.as_u64().unwrap().into_js(&ctx),
            Value::String(x) => x.into_js(&ctx),
            Value::Array(x) => {
                let arr = Array::new(ctx.clone())?;
                for (index, value) in x.into_iter().enumerate() {
                    arr.set(index, SerdeJsonValue(value).into_js(&ctx)?)?;
                }
                Ok(arr.into_value())
            }
            Value::Object(map) => {
                let obj = Object::new(ctx.clone())?;
                for (key, value) in map.into_iter() {
                    obj.set(key, SerdeJsonValue(value).into_js(&ctx)?)?;
                }
                Ok(obj.into_value())
            }
            _ => unimplemented!(),
        }
    }
}
