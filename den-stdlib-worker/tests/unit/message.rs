use rquickjs::{
    ArrayBuffer, AsyncContext, AsyncRuntime, CatchResultExt, CaughtError, Class, Context, FromJs,
    Function, Module, Runtime, Value,
};

use super::Message;
use crate::{
    port::NativePort,
    transport::{Envelope, PortHandle},
};

/// A fresh runtime with `den:worker` installed. One per test: the module
/// keeps its clone hooks in the context userdata, so contexts are not
/// shared.
fn worker_context() -> (Runtime, Context) {
    let runtime = Runtime::new().expect("runtime");
    let context = Context::full(&runtime).expect("context");
    context.with(|ctx| {
        let install = || {
            let (_, evaluated) =
                Module::evaluate_def::<crate::js_worker, _>(ctx.clone(), "den:worker")?;
            evaluated.finish::<()>()
        };
        install()
            .catch(&ctx)
            .map_err(|err| err.to_string())
            .expect("den:worker evaluates");
    });
    (runtime, context)
}

/// The `name` of whatever a failed clone threw. A `DOMException` is not a
/// `JS_CLASS_ERROR`, so rquickjs catches it as a plain thrown value and its
/// `Display` says nothing useful.
fn thrown_name(error: CaughtError<'_>) -> String {
    match error {
        CaughtError::Value(value) => {
            value
                .as_object()
                .and_then(|object| object.get::<_, String>("name").ok())
                .unwrap_or_else(|| "a value that is not a DOMException".to_owned())
        }
        other => other.to_string(),
    }
}

/// Evaluate `source` — an expression — with `den:worker` installed.
fn eval<T>(source: &str) -> Result<T, String>
where
    T: for<'js> FromJs<'js>,
{
    let (_runtime, context) = worker_context();
    context.with(|ctx| {
        ctx.eval::<T, _>(source)
            .catch(&ctx)
            .map_err(|err| err.to_string())
    })
}

/// `"DataCloneError"` when cloning `expression` throws the right thing,
/// and whatever went wrong otherwise — so a failing assertion names it.
fn clone_failure(expression: &str) -> String {
    eval::<String>(&format!(
        r#"(() => {{
                 try {{ structuredClone({expression}); return "no throw"; }}
                 catch (error) {{
                   return error instanceof DOMException && error instanceof Error
                     ? error.name : `wrong error: ${{error}}`;
                 }}
               }})()"#
    ))
    .expect("the snippet evaluates")
}

/// Assert a JS expression over `structuredClone`'s result is true.
fn assert_clone(source: &str) {
    assert_eq!(eval::<bool>(source), Ok(true), "{source}");
}

#[test]
fn primitives_round_trip_including_negative_zero_and_nan() {
    assert_clone(include_str!(
        "../fixtures/unit/message/primitives_round_trip_including_negative_zero_and_nan.js"
    ));
}

#[test]
fn boxed_primitives_stay_boxed() {
    assert_clone(include_str!(
        "../fixtures/unit/message/boxed_primitives_stay_boxed.js"
    ));
}

#[test]
fn date_preserves_its_time_value() {
    assert_clone(include_str!(
        "../fixtures/unit/message/date_preserves_its_time_value.js"
    ));
}

#[test]
fn regexp_preserves_source_and_flags_and_resets_last_index() {
    // `lastIndex` is deliberately not carried: HTML step 12 clones source
    // and flags only, and so does `JS_WriteRegExp`.
    assert_clone(include_str!(
        "../fixtures/unit/message/regexp_preserves_source_and_flags_and_resets_last_index.js"
    ));
}

/// Regression test for a quickjs-ng reader bug: `JS_ReadRegExp`
/// (quickjs.c:39435) is the one reader that does not register the object it
/// built in the reference table, while the writer registers every object —
/// so before clone.js started rebuilding RegExp from its parts, one RegExp
/// anywhere in a graph shifted every later back-reference by one and the
/// read failed outright. Two views over one buffer are the cheapest
/// back-reference there is.
#[test]
fn a_regexp_does_not_shift_the_back_references_that_follow_it() {
    assert_clone(include_str!(
        "../fixtures/unit/message/a_regexp_does_not_shift_the_back_references_that_follow_it.js"
    ));
}

#[test]
fn array_buffer_round_trips_as_a_copy() {
    assert_clone(include_str!(
        "../fixtures/unit/message/array_buffer_round_trips_as_a_copy.js"
    ));
}

#[test]
fn every_typed_array_kind_round_trips() {
    // Float16Array is quickjs-ng's; the check skips what a build lacks.
    assert_clone(include_str!(
        "../fixtures/unit/message/every_typed_array_kind_round_trips.js"
    ));
}

#[test]
fn typed_array_preserves_offset_length_and_buffer_aliasing() {
    assert_clone(include_str!(
        "../fixtures/unit/message/typed_array_preserves_offset_length_and_buffer_aliasing.js"
    ));
}

#[test]
fn data_view_round_trips_and_shares_its_buffer_with_a_sibling_view() {
    assert_clone(include_str!(
        "../fixtures/unit/message/data_view_round_trips_and_shares_its_buffer_with_a_sibling_view.\
         js"
    ));
}

#[test]
fn map_and_set_round_trip_preserving_insertion_order() {
    assert_clone(include_str!(
        "../fixtures/unit/message/map_and_set_round_trip_preserving_insertion_order.js"
    ));
}

#[test]
fn arrays_and_nested_objects_round_trip() {
    assert_clone(include_str!(
        "../fixtures/unit/message/arrays_and_nested_objects_round_trip.js"
    ));
}

#[test]
fn bigint_round_trips_beyond_the_i64_boundary() {
    // `BigInt::to_i64` silently returns 0 outside i64, so nothing in the
    // pipeline is allowed to go through it.
    assert_clone(include_str!(
        "../fixtures/unit/message/bigint_round_trips_beyond_the_i64_boundary.js"
    ));
}

#[test]
fn every_error_subtype_round_trips_with_message_and_stack() {
    assert_clone(include_str!(
        "../fixtures/unit/message/every_error_subtype_round_trips_with_message_and_stack.js"
    ));
}

#[test]
fn error_subclass_degrades_to_error_and_cause_survives() {
    assert_clone(include_str!(
        "../fixtures/unit/message/error_subclass_degrades_to_error_and_cause_survives.js"
    ));
}

#[test]
fn error_without_an_own_message_gets_none() {
    assert_clone(include_str!(
        "../fixtures/unit/message/error_without_an_own_message_gets_none.js"
    ));
}

#[test]
fn dom_exception_round_trips_preserving_name_and_code() {
    assert_clone(include_str!(
        "../fixtures/unit/message/dom_exception_round_trips_preserving_name_and_code.js"
    ));
}

#[test]
fn cycles_and_shared_references_are_preserved() {
    assert_clone(include_str!(
        "../fixtures/unit/message/cycles_and_shared_references_are_preserved.js"
    ));
}

#[test]
fn a_shared_error_reachable_by_two_paths_stays_one_error() {
    // Proves the tagged replacements join the serialiser's reference table.
    assert_clone(include_str!(
        "../fixtures/unit/message/a_shared_error_reachable_by_two_paths_stays_one_error.js"
    ));
}

#[test]
fn a_shared_object_used_as_a_map_key_and_value_stays_one_object() {
    assert_clone(include_str!(
        "../fixtures/unit/message/a_shared_object_used_as_a_map_key_and_value_stays_one_object.js"
    ));
}

#[test]
fn getters_are_invoked_once_and_become_data_properties() {
    assert_clone(include_str!(
        "../fixtures/unit/message/getters_are_invoked_once_and_become_data_properties.js"
    ));
}

#[test]
fn symbol_keys_are_dropped_and_the_prototype_is_flattened() {
    assert_clone(include_str!(
        "../fixtures/unit/message/symbol_keys_are_dropped_and_the_prototype_is_flattened.js"
    ));
}

#[test]
fn array_holes_become_undefined_and_non_index_properties_are_dropped() {
    // Both are deliberate v1 divergences from the spec, inherited from
    // `JS_WriteArray`: fixing them costs a full property walk for no
    // observable gain. Pinned here so a change is a decision, not a
    // surprise.
    assert_clone(include_str!(
        "../fixtures/unit/message/\
         array_holes_become_undefined_and_non_index_properties_are_dropped.js"
    ));
}

#[test]
fn a_map_with_a_live_iterator_parked_past_a_deleted_key_round_trips_intact() {
    // quickjs-ng's `js_map_write` announces `record_count` entries but
    // writes every record, zombies included, which desynchronises the whole
    // stream and eats the *sibling* property. The pre-pass rebuilds every
    // Map and Set to dodge it (docs/research/10 §4.4).
    assert_clone(include_str!(
        "../fixtures/unit/message/\
         a_map_with_a_live_iterator_parked_past_a_deleted_key_round_trips_intact.js"
    ));
}

#[test]
fn a_transferred_buffer_arrives_with_its_bytes_and_leaves_the_source_detached() {
    assert_clone(include_str!(
        "../fixtures/unit/message/\
         a_transferred_buffer_arrives_with_its_bytes_and_leaves_the_source_detached.js"
    ));
}

#[test]
fn a_failed_clone_leaves_transferred_buffers_attached() {
    // Spec order: serialise first, detach only after it succeeded.
    assert_clone(include_str!(
        "../fixtures/unit/message/a_failed_clone_leaves_transferred_buffers_attached.js"
    ));
}

#[test]
fn a_duplicate_in_the_transfer_list_throws_data_clone_error() {
    assert_eq!(
        eval::<String>(include_str!(
            "../fixtures/unit/message/a_duplicate_in_the_transfer_list_throws_data_clone_error.js"
        )),
        Ok("DataCloneError".to_owned())
    );
}

#[test]
fn a_detached_buffer_in_the_transfer_list_throws_data_clone_error_and_leaves_no_pending_exception()
{
    // `ArrayBuffer::from_value` on a detached buffer arms a pending
    // TypeError that would surface at the next unrelated call, so the
    // detach probe is the `detached` getter — and the follow-up call here
    // is what proves it.
    assert_eq!(
        eval::<String>(include_str!(
            "../fixtures/unit/message/a_detached_buffer_in_the_transfer_list_throws_data_clone_error_and_leaves_no_pending_exception.js"
        )),
        Ok("DataCloneError:1".to_owned())
    );
}

#[test]
fn a_non_transferable_in_the_transfer_list_throws_data_clone_error() {
    assert_eq!(
        eval::<String>(
            r#"(() => {
                     try { structuredClone({}, { transfer: [{}] }); return "no throw"; }
                     catch (error) { return error.name; }
                   })()"#
        ),
        Ok("DataCloneError".to_owned())
    );
}

#[test]
fn every_forbidden_type_throws_data_clone_error() {
    for expression in [
        "Symbol('x')",
        "Object(Symbol('x'))",
        "() => {}",
        "class Nope {}",
        "new Proxy({}, {})",
        "Promise.resolve()",
        "new WeakMap()",
        "new WeakSet()",
        "new WeakRef({})",
        "new FinalizationRegistry(() => {})",
        "(function* generate() {})()",
        "(function () { return arguments; })()",
    ] {
        assert_eq!(clone_failure(expression), "DataCloneError", "{expression}");
    }
}

#[test]
fn a_detached_buffer_inside_the_graph_throws_data_clone_error() {
    assert_eq!(
        clone_failure("(() => { const b = new ArrayBuffer(4); b.transfer(); return { b }; })()"),
        "DataCloneError"
    );
}

#[test]
fn a_proxy_is_refused_without_running_a_single_trap() {
    assert_clone(include_str!(
        "../fixtures/unit/message/a_proxy_is_refused_without_running_a_single_trap.js"
    ));
}

#[test]
fn a_message_round_trips_between_two_runtimes() {
    // The real topology: serialise under one runtime's lock, rebuild under
    // another's. Only the `Message` crosses.
    let source =
        include_str!("../fixtures/unit/message/a_message_round_trips_between_two_runtimes.js");
    let (_sender_runtime, sender) = worker_context();
    let message = sender.with(|ctx| {
        let value: Value<'_> = ctx.eval(source).expect("the fixture evaluates");
        Message::serialize(&ctx, value, vec![], vec![])
            .catch(&ctx)
            .map_err(|err| err.to_string())
            .expect("the fixture serialises")
    });

    let (_receiver_runtime, receiver) = worker_context();
    let summary: String = receiver.with(|ctx| {
        let (value, ports) = message
            .deserialize(&ctx)
            .catch(&ctx)
            .map_err(|err| err.to_string())
            .expect("the message deserialises");
        assert!(ports.is_empty());
        ctx.globals().set("received", value).expect("global set");
        ctx.eval::<String, _>(
            r#"[received.when.getTime(), received.why instanceof TypeError,
                    received.why.message, received.pair.get("k") === received.also,
                    received.bytes.join("-"), received.self === received].join()"#,
        )
        .catch(&ctx)
        .map_err(|err| err.to_string())
        .expect("the checks evaluate")
    });
    assert_eq!(summary, "1000,true,boom,true,1-2-3,true");
}
#[test]
fn a_transferred_port_moves_its_channel_and_detaches_the_source() {
    // The JS `MessagePort` wrapper is the port prelude's business; what is
    // owned here is that the channel end travels with the message and the
    // sender's port is left detached, so a second transfer fails.
    let (moved, peer) = PortHandle::pair();
    let (_sender_runtime, sender) = worker_context();
    let (message, resent) = sender.with(|ctx| {
        let port = Class::instance(ctx.clone(), NativePort::from_handle(moved))
            .expect("the native port instantiates");
        let message = Message::serialize(&ctx, Value::new_null(ctx.clone()), vec![], vec![
            port.clone(),
        ])
        .catch(&ctx)
        .map_err(|err| err.to_string())
        .expect("the port transfers");
        assert!(!port.borrow().is_open(), "the source port is detached");
        let resent = Message::serialize(&ctx, Value::new_null(ctx.clone()), vec![], vec![port])
            .catch(&ctx)
            .map_err(thrown_name)
            .unwrap_err();
        (message, resent)
    });
    assert_eq!(resent, "DataCloneError");

    let (_receiver_runtime, receiver) = worker_context();
    receiver.with(|ctx| {
        let (_, ports) = message
            .deserialize(&ctx)
            .catch(&ctx)
            .map_err(|err| err.to_string())
            .expect("the message deserialises");
        assert_eq!(ports.len(), 1, "exactly one port arrived");
        let port = &ports[0];
        assert!(port.borrow().is_open());
        port.borrow()
            .take_handle()
            .expect("the port still holds its channel")
            .send(Envelope::Close)
            .expect("the peer is still listening");
    });

    let mut peer = peer;
    assert!(matches!(
        peer.take_receiver()
            .expect("the peer keeps its inbox")
            .try_recv(),
        Ok(Envelope::Close)
    ));
}

#[test]
fn the_same_port_twice_in_the_transfer_list_throws_data_clone_error() {
    let (handle, _peer) = PortHandle::pair();
    let (_runtime, context) = worker_context();
    let failure = context.with(|ctx| {
        let port = Class::instance(ctx.clone(), NativePort::from_handle(handle))
            .expect("the native port instantiates");
        Message::serialize(&ctx, Value::new_null(ctx.clone()), vec![], vec![
            port.clone(),
            port,
        ])
        .catch(&ctx)
        .map_err(thrown_name)
        .unwrap_err()
    });
    assert_eq!(failure, "DataCloneError");
}

#[test]
fn a_getter_that_invalidates_the_transfer_list_during_the_walk_transfers_nothing() {
    // The transfer list is validated before the serialisation walk, but the
    // walk runs every getter in the graph, and a getter is free to close a
    // port or detach a buffer that was valid a moment earlier. Revalidating
    // afterwards is what keeps the refusal atomic: without it the buffer is
    // detached first and the port's refusal arrives too late to undo it.
    assert_eq!(
        eval::<String>(include_str!(
            "../fixtures/unit/message/\
             a_getter_that_invalidates_the_transfer_list_during_the_walk_transfers_nothing.js"
        )),
        Ok("DataCloneError:false:transferable".to_owned())
    );
}

#[test]
fn a_view_left_out_of_bounds_by_a_shrunk_resizable_buffer_throws_data_clone_error() {
    // quickjs' writer records the view's stale offset without complaint and
    // the *reader* is the one that refuses it ("invalid offset"), so the
    // failure used to surface as a `RangeError` — or, across a worker, as a
    // far-side `messageerror`.
    assert_eq!(
        clone_failure(include_str!(
            "../fixtures/unit/message/\
             a_view_left_out_of_bounds_by_a_shrunk_resizable_buffer_throws_data_clone_error.js"
        )),
        "DataCloneError"
    );
}

/// The DataView half of the same rule. This one always threw
/// synchronously — but with quickjs's own `TypeError: ArrayBuffer is
/// detached or resized`, escaping from the `byteOffset` read the DataView
/// branch of the walk does, where HTML asks for a DataCloneError.
#[test]
fn an_out_of_bounds_data_view_throws_data_clone_error_rather_than_a_type_error() {
    assert_eq!(
        clone_failure(include_str!(
            "../fixtures/unit/message/\
             an_out_of_bounds_data_view_throws_data_clone_error_rather_than_a_type_error.js"
        )),
        "DataCloneError"
    );
}

/// The rule must not swallow the legitimately empty view it resembles:
/// quickjs reports byteOffset 0 and length 0 for an out-of-bounds typed
/// array, which is exactly what a zero-length view in bounds reports too.
#[test]
fn a_zero_length_view_in_bounds_still_clones() {
    assert_clone(include_str!(
        "../fixtures/unit/message/a_zero_length_view_in_bounds_still_clones.js"
    ));
}

#[test]
fn a_buffer_sealed_with_an_own_transfer_property_is_refused_and_left_intact() {
    // `WebAssembly.Memory#buffer` is sealed exactly this way
    // (den-stdlib-wasm/src/memory.rs `seal_against_transfer`), which is how
    // den spells `[[ArrayBufferDetachKey]]`. Transferring one would detach
    // the wasm linear memory out from under a live instance, so the guard
    // has to hold for any buffer carrying an own `transfer`.
    assert_eq!(
        eval::<String>(include_str!(
            "../fixtures/unit/message/\
             a_buffer_sealed_with_an_own_transfer_property_is_refused_and_left_intact.js"
        )),
        Ok("DataCloneError:false:8".to_owned())
    );
}

/// Transfer is all-or-nothing. A refusal that `validate_ports` misses is
/// found by `take_handle` instead — halfway through the mutation, with the
/// buffers already detached and the earlier ports already moved out, and
/// nothing hands those back. A started port is exactly such a refusal.
#[tokio::test]
async fn a_started_port_is_refused_before_any_buffer_or_port_is_transferred() {
    let runtime = AsyncRuntime::new().expect("runtime");
    let context = AsyncContext::full(&runtime).await.expect("context");
    // The peers are held for the length of the test: a dropped peer closes
    // the port, which would make `is_open` false for the wrong reason.
    let (first, _first_peer) = PortHandle::pair();
    let (second, _second_peer) = PortHandle::pair();
    let outcome: String = context
        .with(|ctx| {
            let run = || -> Result<String, rquickjs::Error> {
                let (_, evaluated) =
                    Module::evaluate_def::<crate::js_worker, _>(ctx.clone(), "den:worker")?;
                evaluated.finish::<()>()?;

                let moved = Class::instance(ctx.clone(), NativePort::from_handle(first))?;
                let started = Class::instance(ctx.clone(), NativePort::from_handle(second))?;
                let noop = Function::new(ctx.clone(), || {})?;
                started
                    .borrow()
                    .start(ctx.clone(), noop.clone(), noop.clone(), noop.clone());

                let buffer = ArrayBuffer::new_copy(ctx.clone(), [1u8, 2, 3, 4])?;
                let failure = Message::serialize(
                    &ctx,
                    Value::new_null(ctx.clone()),
                    vec![buffer.as_value().clone()],
                    vec![moved.clone(), started],
                )
                .catch(&ctx)
                .map_err(thrown_name)
                .expect_err("a started port cannot be transferred");
                Ok(format!(
                    "{failure}:{}:{}",
                    moved.borrow().is_open(),
                    buffer.as_object().get::<_, bool>("detached")?
                ))
            };
            run().catch(&ctx).map_err(|err| err.to_string())
        })
        .await
        .expect("the fixture runs");
    // The port earlier in the list kept its channel and the buffer is still
    // attached: the refusal cost the caller nothing.
    assert_eq!(outcome, "DataCloneError:true:false");
}

#[test]
fn an_own_proto_data_property_survives_without_reparenting_the_clone() {
    // What `JSON.parse('{"__proto__":1}')` produces: an own *data* property
    // whose name is the one `Object.prototype` exposes as an accessor. Built
    // with assignment, the accessor swallows it and the property vanishes
    // from every cloned object; built with CreateDataProperty it survives,
    // and the clone's prototype is still `Object.prototype`.
    assert_eq!(
        eval::<String>(include_str!(
            "../fixtures/unit/message/\
             an_own_proto_data_property_survives_without_reparenting_the_clone.js"
        )),
        Ok("true,true,true,2".to_owned())
    );
}

#[test]
fn a_poisoned_object_prototype_accessor_neither_sees_nor_swallows_a_cloned_property() {
    // An inherited setter intercepts [[Set]] on a fresh output object: the
    // data reaches the attacker and no own property is created, so the
    // clone silently loses the key. `cause` covers both halves at once —
    // the sender's tag object and the receiver's rebuilt `Error` are each
    // given one, and neither has it as an own property beforehand.
    assert_eq!(
        eval::<String>(include_str!(
            "../fixtures/unit/message/\
             a_poisoned_object_prototype_accessor_neither_sees_nor_swallows_a_cloned_property.js"
        )),
        Ok("0,true,5,true,why".to_owned())
    );
}

#[test]
fn a_key_deleted_by_an_earlier_getter_is_omitted_rather_than_cloned_as_undefined() {
    // The walk snapshots the key list, so a key deleted while it runs is
    // still in that snapshot; ownership is re-checked to keep it out of the
    // output instead of reviving it as an own `undefined`.
    assert_eq!(
        eval::<String>(include_str!(
            "../fixtures/unit/message/\
             a_key_deleted_by_an_earlier_getter_is_omitted_rather_than_cloned_as_undefined.js"
        )),
        Ok("false,1,first+trap".to_owned())
    );
}

#[test]
fn every_forbidden_type_names_itself_in_the_data_clone_error_message() {
    // `every_forbidden_type_throws_data_clone_error` passes with the JS
    // pre-screen deleted, because the writer refuses these too — with a
    // `TypeError` that Rust re-tags as "the value could not be cloned: …".
    // The message is the only evidence of which of the two refused, so it
    // is what pins the pre-screen.
    for (expression, message) in [
        ("Promise.resolve()", "Promise could not be cloned."),
        ("new WeakMap()", "WeakMap could not be cloned."),
        ("new WeakSet()", "WeakSet could not be cloned."),
        ("new WeakRef({})", "WeakRef could not be cloned."),
        (
            "new FinalizationRegistry(() => {})",
            "FinalizationRegistry could not be cloned.",
        ),
        (
            "new SharedArrayBuffer(8)",
            "SharedArrayBuffer could not be cloned.",
        ),
        ("new Proxy({}, {})", "#<Proxy> could not be cloned."),
        ("Symbol('x')", "Symbol(x) could not be cloned."),
        ("function named() {}", "function named could not be cloned."),
        ("() => {}", "function (anonymous) could not be cloned."),
    ] {
        assert_eq!(
            eval::<String>(&format!(
                r#"(() => {{
                         try {{ structuredClone({expression}); return "no throw"; }}
                         catch (error) {{ return `${{error.name}}: ${{error.message}}`; }}
                       }})()"#
            )),
            Ok(format!("DataCloneError: {message}")),
            "{expression}"
        );
    }
}
