use ext_php_rs::binary::Binary;
use ext_php_rs::prelude::*;
use ext_php_rs::types::Zval;
use ext_php_rs::{exception::PhpException, zend::ce};

use std::collections::HashMap;

mod runtime;

pub use crate::runtime::JSRuntime;

#[derive(ZvalConvert, Debug, Clone, PartialEq)]
pub enum PHPValue {
    String(String),
    None,
    Boolean(bool),
    Float(f64),
    Integer(i64),
    Array(Vec<PHPValue>),
    Object(HashMap<String, PHPValue>),
}

impl PHPValue {
    pub fn from(result: v8::Local<v8::Value>, scope: &mut v8::HandleScope) -> Self {
        if result.is_string() {
            return PHPValue::String(result.to_rust_string_lossy(scope));
        }
        if result.is_null_or_undefined() {
            return PHPValue::None;
        }
        if result.is_boolean() {
            return PHPValue::Boolean(result.boolean_value(scope));
        }
        if result.is_int32() {
            return PHPValue::Integer(result.integer_value(scope).unwrap());
        }
        if result.is_number() {
            return PHPValue::Float(result.number_value(scope).unwrap());
        }
        if result.is_array() {
            let array = v8::Local::<v8::Array>::try_from(result).unwrap();
            let mut vec: Vec<PHPValue> = Vec::new();
            for index in 0..array.length() {
                vec.push(PHPValue::from(
                    array.get_index(scope, index).unwrap(),
                    scope,
                ));
            }
            return PHPValue::Array(vec);
        }
        if result.is_function() {
            return PHPValue::String(String::from("Function"));
        }
        if result.is_object() {
            let object = v8::Local::<v8::Object>::try_from(result).unwrap();
            let properties = object.get_own_property_names(scope).unwrap();
            let mut hashmap: HashMap<String, PHPValue> = HashMap::new();
            for index in 0..properties.length() {
                let key = properties.get_index(scope, index).unwrap();
                let value = object.get(scope, key).unwrap();
                hashmap.insert(
                    key.to_rust_string_lossy(scope),
                    PHPValue::from(value, scope),
                );
            }
            return PHPValue::Object(hashmap);
        }
        PHPValue::String(result.to_rust_string_lossy(scope))
    }
}

#[php_class]
#[extends(ce::exception())]
#[derive(Default)]
pub struct V8JsScriptException;

pub fn js_value_from_zval<'a>(
    scope: &mut v8::HandleScope<'a>,
    zval: &'_ Zval,
) -> v8::Local<'a, v8::Value> {
    if zval.is_string() {
        return v8::String::new(scope, zval.str().unwrap()).unwrap().into();
    }
    if zval.is_long() || zval.is_double() {
        return v8::Number::new(scope, zval.double().unwrap()).into();
    }
    if zval.is_bool() {
        return v8::Boolean::new(scope, zval.bool().unwrap()).into();
    }
    if zval.is_true() {
        return v8::Boolean::new(scope, true).into();
    }
    if zval.is_false() {
        return v8::Boolean::new(scope, false).into();
    }
    if zval.is_null() {
        return v8::null(scope).into();
    }
    if zval.is_array() {
        let zend_array = zval.array().unwrap();
        let mut values: Vec<v8::Local<'_, v8::Value>> = Vec::new();
        let mut keys: Vec<v8::Local<'_, v8::Name>> = Vec::new();
        let mut has_string_keys = false;
        for (index, key, elem) in zend_array.iter() {
            let key = match key {
                Some(key) => {
                    has_string_keys = true;
                    key
                }
                None => index.to_string(),
            };
            keys.push(v8::String::new(scope, key.as_str()).unwrap().into());
            values.push(js_value_from_zval(scope, elem));
        }

        if has_string_keys {
            let null: v8::Local<v8::Value> = v8::null(scope).into();
            return v8::Object::with_prototype_and_properties(scope, null, &keys[..], &values[..])
                .into();
        } else {
            return v8::Array::new_with_elements(scope, &values[..]).into();
        }
    }
    v8::null(scope).into()
}

#[php_class]
pub struct V8Js {
    global_name: String,
    runtime: JSRuntime,
}

#[php_impl(rename_methods = "camelCase")]
impl V8Js {
    pub fn __construct(
        object_name: Option<String>,
        _variables: Option<HashMap<String, String>>,
        _extensions: Option<HashMap<String, String>>,
        _report_uncaight_exceptions: Option<bool>,
        snapshot_blob: Option<Binary<u8>>,
    ) -> Self {
        let global_name = match object_name {
            Some(name) => name,
            None => String::from("PHP"),
        };
        let snapshot_blob = match snapshot_blob {
            Some(snapshot_blob) => Some(snapshot_blob.as_slice().to_vec()),
            None => None,
        };
        let mut runtime = JSRuntime::new(snapshot_blob);
        let object: v8::Global<v8::Value>;
        {
            let scope = &mut runtime.handle_scope();
            let o: v8::Local<v8::Value> = v8::Object::new(scope).into();
            object = v8::Global::new(scope, o);
        }
        runtime.add_global(global_name.as_str(), object);
        runtime.add_global_function("var_dump", php_callback_var_dump);
        runtime.add_global_function("print", php_callback_var_dump);
        runtime.add_global_function("exit", php_callback_exit);
        runtime.add_global_function("sleep", php_callback_sleep);
        V8Js {
            runtime,
            global_name,
        }
    }
    pub fn set_module_loader(&mut self, _callable: &Zval) {
        // let mut loader = self
        //     .runtime
        //     .isolate
        //     .get_slot::<Rc<RefCell<ModuleLoader>>>()
        //     .unwrap()
        //     .borrow_mut();
        // let callable = callable.shallow_clone();
        // loader.callback = Some(callable);
        // self.commonjs_module_loader = Some(callable)
    }

    pub fn execute_string(
        &mut self,
        string: String,
        identifier: Option<String>,
        _flags: Option<String>,
        time_limit: Option<u64>,
        memory_limit: Option<u64>,
    ) -> Result<PHPValue, PhpException> {
        let result = self.runtime.execute_string(
            string.as_str(),
            identifier,
            _flags,
            time_limit,
            memory_limit,
        );

        match result {
            Ok(result) => {
                match result {
                    Some(result) => {
                        let mut scope = &mut self.runtime.handle_scope();
                        let local = v8::Local::new(scope, result);
                        Ok(PHPValue::from(local, &mut scope))
                    },
                    None => Ok(PHPValue::None),
                }
            }
            _ => Err(PhpException::default(String::from("Exception"))),
        }
    }

    pub fn __set(&mut self, property: &str, value: &Zval) {
        {
            let global = self.runtime.get_global(self.global_name.as_str());
            let global = match global {
                Some(global) => global,
                None => return (),
            };
            let mut scope = self.runtime.handle_scope();
            let global = v8::Local::new(&mut scope, global);
            let global: v8::Local<v8::Object> = v8::Local::<v8::Object>::try_from(global).unwrap();
            let property_name = v8::String::new(&mut scope, property).unwrap();

            let js_value;
            if  value.is_callable() {
                let function_builder: v8::FunctionBuilder<v8::Function> = v8::FunctionBuilder::new(php_callback);
                let function_builder = function_builder.data(property_name.into());
                let function: v8::Local<v8::Value> = function_builder.build(&mut scope).unwrap().into();
                js_value = function;
            } else {
                js_value = js_value_from_zval(&mut scope, value);
            }
            global.set(&mut scope, property_name.into(), js_value);
        }
        if  value.is_callable() {
            let value = value.shallow_clone();
            self.runtime.add_callback(property, value);
        }
    }

    pub fn create_snapshot(source: String) -> Option<Zval> {
        let snapshot = JSRuntime::create_snapshot(source)?;
        let mut zval = Zval::new();
        zval.set_binary(snapshot);
        Some(zval)
    }
}
#[derive(Debug)]
struct StartupData {
    data: *const char,
    raw_size: std::os::raw::c_int,
}

pub fn php_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let isolate: &mut v8::Isolate = scope.as_mut();
    let state = JSRuntime::state(isolate);
    let state = state.borrow_mut();
    let callback_name = args.data().unwrap().to_rust_string_lossy(scope);
    let callback = state.callbacks.get(&callback_name);
    if callback.is_none() {
        println!("callback not found {:#?}", callback_name);
        return;
    }
    let callback = callback.unwrap();

    if callback.is_callable() == false {
        println!("callback not callable {:#?}", callback);
        return;
    }

    let mut php_args: Vec<PHPValue> = Vec::new();
    let mut php_arg_refs: Vec<&dyn ext_php_rs::convert::IntoZvalDyn> = Vec::new();

    for index in 0..args.length() {
        let v = PHPValue::from(args.get(index), scope);
        php_args.push(v);
    }
    for index in &php_args {
        php_arg_refs.push(index);
    }
    let return_value = callback.try_call(php_arg_refs).unwrap();
    let return_value_js = js_value_from_zval(scope, &return_value);
    rv.set(return_value_js)
}

pub fn php_callback_sleep(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let sleep = ext_php_rs::types::ZendCallable::try_from_name("sleep").unwrap();
    let arg = PHPValue::from(args.get(0), scope);
    let mut php_arg_refs: Vec<&dyn ext_php_rs::convert::IntoZvalDyn> = Vec::new();
    php_arg_refs.push(&arg);
    let result = sleep.try_call(php_arg_refs);
    let result = match result {
        Ok(result) => result,
        Err(_) => Zval::new(), // todo: JS error objects?
    };
    let result = js_value_from_zval(scope, &result);
    rv.set(result);
}

pub fn php_callback_var_dump(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let var_dump = ext_php_rs::types::ZendCallable::try_from_name("var_dump").unwrap();
    let arg = PHPValue::from(args.get(0), scope);
    let mut php_arg_refs: Vec<&dyn ext_php_rs::convert::IntoZvalDyn> = Vec::new();
    php_arg_refs.push(&arg);
    let result = var_dump.try_call(php_arg_refs);
    let result = match result {
        Ok(result) => result,
        Err(_) => Zval::new(), // todo: JS error objects?
    };
    let result = js_value_from_zval(scope, &result);
    rv.set(result);
}

pub fn php_callback_exit(
    scope: &mut v8::HandleScope,
    _args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    if scope.is_execution_terminating() {
        return ();
    }

    // There's no way to immediately terminate execution in V8 so
    // we have to spin it's wheels with an inf. loop until it terminates.
    let script;
    {
        let code = v8::String::new(scope, "for(;;);").unwrap();
        script = v8::Script::compile(scope, code, None).unwrap();
    }
    scope.terminate_execution();
    script.run(scope);
}

#[php_module]
pub fn get_module(module: ModuleBuilder) -> ModuleBuilder {
    module
}

#[cfg(test)]
mod integration {
    use std::process::Command;
    use std::sync::Once;

    static BUILD: Once = Once::new();

    fn setup() {
        BUILD.call_once(|| {
            assert!(Command::new("cargo")
                .arg("build")
                .output()
                .expect("failed to build extension")
                .status
                .success());
        });
    }

    pub fn run_php(file: &str) -> bool {
        setup();
        let output = Command::new("php")
            .arg(format!(
                "-dextension=target/debug/libphp_v8.{}",
                std::env::consts::DLL_EXTENSION
            ))
            .arg("-n")
            .arg(format!("tests/{}", file))
            .output()
            .expect("failed to run php file");
        if output.status.success() {
            true
        } else {
            panic!(
                "
                status: {}
                stdout: {}
                stderr: {}
                ",
                output.status,
                String::from_utf8(output.stdout).unwrap(),
                String::from_utf8(output.stderr).unwrap()
            );
        }
    }
    #[test]
    fn snapshot() {
        run_php("snapshot.php");
    }

    #[test]
    fn execute_string() {
        run_php("execute_string.php");
    }

    #[test]
    fn php_bridge() {
        run_php("php_bridge.php");
    }
}
