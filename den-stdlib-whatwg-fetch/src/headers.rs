use indexmap::IndexMap;
use rquickjs::{
    Array, Class, Coerced, Ctx, Exception, Filter, FromJs, Function, IntoJs, Iterable, JsLifetime,
    Object, Result, Value,
    atom::PredefinedAtom,
    class::Trace,
    function::{Opt, This},
};

#[derive(Trace, JsLifetime, Clone)]
#[rquickjs::class]
pub struct Headers {
    #[qjs(skip_trace)]
    pub(crate) map: IndexMap<String, String>,
}

impl Headers {
    pub(crate) fn pairs(&self) -> Vec<(String, String)> {
        self.map
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect()
    }

    pub(crate) fn from_pairs(
        pairs: impl IntoIterator<Item = (impl AsRef<str>, impl AsRef<str>)>,
    ) -> Self {
        let mut headers = Self {
            map: IndexMap::new(),
        };
        for (name, value) in pairs {
            headers.append_combined(
                name.as_ref().to_ascii_lowercase(),
                value.as_ref().to_string(),
            );
        }
        headers
    }

    fn is_valid_name(name: &str) -> bool {
        !name.is_empty()
            && name.bytes().all(|byte| {
                matches!(
                    byte,
                    b'0'..=b'9'
                        | b'a'..=b'z'
                        | b'A'..=b'Z'
                        | b'!'
                        | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
            })
    }

    fn normalize_name(ctx: &Ctx<'_>, name: &str) -> Result<String> {
        if !Self::is_valid_name(name) {
            return Err(Exception::throw_type(
                ctx,
                &format!("Invalid character in header field name: \"{name}\""),
            ));
        }
        Ok(name.to_ascii_lowercase())
    }

    fn append_combined(&mut self, name: String, value: String) {
        match self.map.get(&name) {
            Some(old) => {
                let combined = format!("{old}, {value}");
                self.map.insert(name, combined);
            }
            None => {
                self.map.insert(name, value);
            }
        }
    }

    fn fill<'js>(&mut self, ctx: &Ctx<'js>, init: Value<'js>) -> Result<()> {
        let Some(object) = init.as_object() else {
            return Ok(());
        };
        if let Some(other) = Class::<Headers>::from_object(object) {
            self.map = other.borrow().map.clone();
            return Ok(());
        }
        if object.is_array() {
            return self.fill_pairs(ctx, object);
        }
        for name in object.own_keys::<String>(Filter::new().string()) {
            let name = name?;
            let value = Coerced::<String>::from_js(ctx, object.get(&name)?)?;
            self.append(ctx.clone(), Coerced(name), value)?;
        }
        Ok(())
    }

    fn fill_pairs<'js>(&mut self, ctx: &Ctx<'js>, object: &Object<'js>) -> Result<()> {
        let length: i32 = object.get("length").unwrap_or(0);
        for index in 0..length {
            let entry: Value = object.get(index as u32)?;
            let Some(pair) = entry.as_object() else {
                return Err(Exception::throw_type(
                    ctx,
                    "Expected name/value pair to be length 2, found0",
                ));
            };
            let pair_len: i32 = pair.get("length").unwrap_or(0);
            if pair_len != 2 {
                return Err(Exception::throw_type(
                    ctx,
                    &format!("Expected name/value pair to be length 2, found{pair_len}"),
                ));
            }
            let name = Coerced::<String>::from_js(ctx, pair.get(0)?)?;
            let value = Coerced::<String>::from_js(ctx, pair.get(1)?)?;
            self.append(ctx.clone(), name, value)?;
        }
        Ok(())
    }

    fn entries_iter<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let mut pairs = Vec::with_capacity(self.map.len());
        for (name, value) in &self.map {
            let pair = Array::new(ctx.clone())?;
            pair.set(0, name.clone())?;
            pair.set(1, value.clone())?;
            pairs.push(pair);
        }
        Iterable::from(pairs).into_js(ctx)
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl Headers {
    #[qjs(constructor)]
    pub fn new<'js>(ctx: Ctx<'js>, init: Opt<Value<'js>>) -> Result<Self> {
        let mut headers = Self {
            map: IndexMap::new(),
        };
        if let Some(init) = init
            .0
            .filter(|value| !value.is_null() && !value.is_undefined())
        {
            headers.fill(&ctx, init)?;
        }
        Ok(headers)
    }

    pub fn append(
        &mut self,
        ctx: Ctx<'_>,
        name: Coerced<String>,
        value: Coerced<String>,
    ) -> Result<()> {
        let name = Self::normalize_name(&ctx, &name.0)?;
        self.append_combined(name, value.0);
        Ok(())
    }

    #[qjs(rename = "delete")]
    pub fn r#delete(&mut self, ctx: Ctx<'_>, name: Coerced<String>) -> Result<()> {
        let name = Self::normalize_name(&ctx, &name.0)?;
        self.map.shift_remove(&name);
        Ok(())
    }

    pub fn get<'js>(&self, ctx: Ctx<'js>, name: Coerced<String>) -> Result<Value<'js>> {
        let name = Self::normalize_name(&ctx, &name.0)?;
        match self.map.get(&name) {
            Some(value) => value.clone().into_js(&ctx),
            None => Ok(Value::new_null(ctx)),
        }
    }

    pub fn has(&self, ctx: Ctx<'_>, name: Coerced<String>) -> Result<bool> {
        let name = Self::normalize_name(&ctx, &name.0)?;
        Ok(self.map.contains_key(&name))
    }

    pub fn set(
        &mut self,
        ctx: Ctx<'_>,
        name: Coerced<String>,
        value: Coerced<String>,
    ) -> Result<()> {
        let name = Self::normalize_name(&ctx, &name.0)?;
        self.map.insert(name, value.0);
        Ok(())
    }

    pub fn for_each<'js>(
        this: This<Class<'js, Headers>>,
        callback: Function<'js>,
        this_arg: Opt<Value<'js>>,
        ctx: Ctx<'js>,
    ) -> Result<()> {
        let entries = this
            .0
            .borrow()
            .map
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect::<Vec<_>>();
        let this_arg = this_arg.0.unwrap_or_else(|| Value::new_undefined(ctx));
        for (name, value) in entries {
            callback.call::<_, ()>((This(this_arg.clone()), value, name, this.0.clone()))?;
        }
        Ok(())
    }

    pub fn keys<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        Iterable::from(self.map.keys().cloned().collect::<Vec<_>>()).into_js(&ctx)
    }

    pub fn values<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        Iterable::from(self.map.values().cloned().collect::<Vec<_>>()).into_js(&ctx)
    }

    pub fn entries<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        self.entries_iter(&ctx)
    }

    #[qjs(rename = PredefinedAtom::SymbolIterator)]
    pub fn iterate<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        self.entries_iter(&ctx)
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str {
        "Headers"
    }
}
