use rquickjs::{CatchResultExt as _, Context, FromJs, Function, Object, Result, Runtime, Value};

const EXPOSED: [&str; 2] = ["reportError", "__defineEventHandler"];

/// A fresh runtime whose globals are the native Event family plus the two
/// pieces of plumbing the tests reach for (`reportError`, the handler
/// installer, and the private natives bag).
fn events_context() -> (Runtime, Context) {
    let runtime = Runtime::new().expect("runtime");
    let context = Context::full(&runtime).expect("context");
    context.with(|ctx| {
        let install = || -> Result<()> {
            let natives = Object::new(ctx.clone())?;
            super::install(&ctx, &natives)?;
            super::define_on(&ctx.globals())?;
            let globals = ctx.globals();
            globals.set(
                "reportError",
                Function::new(ctx.clone(), super::report_error)?,
            )?;
            globals.set(
                "__defineEventHandler",
                Function::new(ctx.clone(), super::define_event_handler)?,
            )?;
            globals.set("__natives", natives)?;
            let _ = EXPOSED;
            Ok(())
        };
        install()
            .catch(&ctx)
            .map_err(|err| err.to_string())
            .expect("event classes install");
    });
    (runtime, context)
}

fn eval<T>(source: &str) -> std::result::Result<T, String>
where
    T: for<'js> FromJs<'js>,
{
    let (_runtime, context) = events_context();
    context.with(|ctx| {
        ctx.eval::<T, _>(source)
            .catch(&ctx)
            .map_err(|err| err.to_string())
    })
}

fn trace(source: &str) -> String { eval::<String>(source).expect("script evaluates") }

#[test]
fn listeners_run_in_registration_order_and_once_removes_itself() {
    assert_eq!(
        trace(include_str!(
            "../fixtures/unit/events/listeners_run_in_registration_order_and_once_removes_itself.\
             js"
        )),
        "first,once,last,last"
    );
}

#[test]
fn a_listener_removed_during_dispatch_does_not_run() {
    assert_eq!(
        trace(include_str!(
            "../fixtures/unit/events/a_listener_removed_during_dispatch_does_not_run.js"
        )),
        "first,third"
    );
}

#[test]
fn a_listener_added_during_dispatch_runs_only_on_the_next_one() {
    assert_eq!(
        trace(include_str!(
            "../fixtures/unit/events/a_listener_added_during_dispatch_runs_only_on_the_next_one.js"
        )),
        "first,added"
    );
}

#[test]
fn stop_immediate_propagation_skips_the_remaining_listeners() {
    assert_eq!(
        trace(include_str!(
            "../fixtures/unit/events/stop_immediate_propagation_skips_the_remaining_listeners.js"
        )),
        "first,second|true"
    );
}

#[test]
fn prevent_default_only_cancels_a_cancelable_event() {
    assert_eq!(
        trace(include_str!(
            "../fixtures/unit/events/prevent_default_only_cancels_a_cancelable_event.js"
        )),
        "false,true,false"
    );
}

#[test]
fn a_throwing_listener_is_reported_and_the_next_one_still_runs() {
    assert_eq!(
        trace(include_str!(
            "../fixtures/unit/events/a_throwing_listener_is_reported_and_the_next_one_still_runs.\
             js"
        )),
        "threw,after|true"
    );
}

#[test]
fn a_handle_event_object_is_a_listener_and_null_is_tolerated() {
    assert_eq!(
        trace(include_str!(
            "../fixtures/unit/events/a_handle_event_object_is_a_listener_and_null_is_tolerated.js"
        )),
        "ping:object,function"
    );
}

#[test]
fn dispatch_sets_target_and_phase_and_clears_them_afterwards() {
    assert_eq!(
        trace(include_str!(
            "../fixtures/unit/events/dispatch_sets_target_and_phase_and_clears_them_afterwards.js"
        )),
        "true,true,true,true,false|true,null,0"
    );
}

#[test]
fn re_dispatching_a_dispatching_event_throws_invalid_state_error() {
    assert_eq!(
        trace(include_str!(
            "../fixtures/unit/events/\
             re_dispatching_a_dispatching_event_throws_invalid_state_error.js"
        )),
        "InvalidStateError,true"
    );
}

#[test]
fn an_on_x_handler_keeps_the_position_of_its_first_assignment() {
    assert_eq!(
        trace(include_str!(
            "../fixtures/unit/events/an_on_x_handler_keeps_the_position_of_its_first_assignment.js"
        )),
        "three,two,two,two,five"
    );
}

#[test]
fn an_on_x_slot_reads_back_what_it_stores() {
    assert_eq!(
        trace(include_str!(
            "../fixtures/unit/events/an_on_x_slot_reads_back_what_it_stores.js"
        )),
        "null,true,null,null"
    );
}

#[test]
fn a_handler_returning_false_cancels_and_a_global_onerror_takes_five_arguments() {
    assert_eq!(
        trace(include_str!(
            "../fixtures/unit/events/\
             a_handler_returning_false_cancels_and_a_global_onerror_takes_five_arguments.js"
        )),
        "false|boom,worker.js,3,7,carried|false"
    );
}

#[test]
fn message_event_carries_a_frozen_port_array() {
    assert_eq!(
        trace(include_str!(
            "../fixtures/unit/events/message_event_carries_a_frozen_port_array.js"
        )),
        "42,true,first+second,true,true,true,true,true,0"
    );
}

#[test]
fn error_event_carries_its_fields_and_defaults() {
    assert_eq!(
        trace(include_str!(
            "../fixtures/unit/events/error_event_carries_its_fields_and_defaults.js"
        )),
        "boom,worker.js,3,7,carried,true,0,0,true"
    );
}

#[test]
fn the_classes_look_like_platform_objects() {
    assert_eq!(
        trace(include_str!(
            "../fixtures/unit/events/the_classes_look_like_platform_objects.js"
        )),
        "[object EventTarget],[object MessageEvent],[object \
         ErrorEvent],0,2,1,1,MessageEvent,2,0,true"
    );
}

#[test]
fn a_rust_side_report_goes_through_the_reporter_the_realm_installed() {
    let runtime = Runtime::new().expect("runtime");
    let context = Context::full(&runtime).expect("context");
    let seen = context.with(|ctx| {
        let report = || -> Result<String> {
            let natives = Object::new(ctx.clone())?;
            super::install(&ctx, &natives)?;
            ctx.globals().set("natives", natives)?;
            ctx.eval::<(), _>(include_str!(
                "../fixtures/unit/events/\
                 a_rust_side_report_goes_through_the_reporter_the_realm_installed.js"
            ))?;
            let thrown = ctx.eval::<Value<'_>, _>("new Error('from rust')")?;
            den_stdlib_core::exceptions::report_exception(&ctx, &thrown);
            ctx.globals().get::<_, String>("seen")
        };
        report()
            .catch(&ctx)
            .map_err(|err| err.to_string())
            .expect("the report is routed")
    });
    assert_eq!(seen, "chain:from rust");
}

#[test]
fn dispatching_a_non_event_throws_a_type_error() {
    assert_eq!(
        trace(include_str!(
            "../fixtures/unit/events/dispatching_a_non_event_throws_a_type_error.js"
        )),
        "TypeError"
    );
}

#[test]
fn only_the_runtime_dispatch_marks_an_event_trusted() {
    assert_eq!(
        trace(include_str!(
            "../fixtures/unit/events/only_the_runtime_dispatch_marks_an_event_trusted.js"
        )),
        "true,false,false,false,true,false,false,false"
    );
}

#[test]
fn a_trusted_dispatch_reports_whether_the_event_was_cancelled() {
    assert_eq!(
        trace(include_str!(
            "../fixtures/unit/events/a_trusted_dispatch_reports_whether_the_event_was_cancelled.js"
        )),
        "false,true"
    );
}

#[test]
fn composed_path_and_src_element_follow_the_dispatch() {
    assert_eq!(
        trace(include_str!(
            "../fixtures/unit/events/composed_path_and_src_element_follow_the_dispatch.js"
        )),
        "0,null|1,true,true|0,true"
    );
}

#[test]
fn the_legacy_cancel_bubble_and_return_value_flags_move_one_way_only() {
    assert_eq!(
        trace(include_str!(
            "../fixtures/unit/events/\
             the_legacy_cancel_bubble_and_return_value_flags_move_one_way_only.js"
        )),
        "false,true|false,true|true,false,true|true"
    );
}

#[test]
fn init_event_re_initializes_an_event_but_not_during_a_dispatch() {
    assert_eq!(
        trace(include_str!(
            "../fixtures/unit/events/init_event_re_initializes_an_event_but_not_during_a_dispatch.\
             js"
        )),
        "pong,true,false,false,null|pong|TypeError"
    );
}

#[test]
fn custom_event_carries_its_detail() {
    assert_eq!(
        trace(include_str!(
            "../fixtures/unit/events/custom_event_carries_its_detail.js"
        )),
        "later,pong,null,true,[object CustomEvent]"
    );
}

#[test]
fn report_error_hands_the_value_to_the_realm_s_report_hook() {
    assert_eq!(
        trace(include_str!(
            "../fixtures/unit/events/report_error_hands_the_value_to_the_realm_s_report_hook.js"
        )),
        "1,true,TypeError,1"
    );
}
