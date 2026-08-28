#[test]
fn api_list_has_not_drifted() {
    assert_eq!(crate::API.len(), 17);
    assert_eq!(crate::API, [
        "AbortController",
        "AbortSignal",
        "BroadcastChannel",
        "CustomEvent",
        "ErrorEvent",
        "Event",
        "EventTarget",
        "MessageChannel",
        "MessageEvent",
        "MessagePort",
        "NavigatorUAData",
        "PromiseRejectionEvent",
        "Worker",
        "navigator",
        "performance",
        "reportError",
        "structuredClone",
    ]);
}
