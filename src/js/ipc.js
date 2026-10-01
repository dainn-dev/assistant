// Single seam for Tauri IPC — resolved lazily so tests can stub
// window.__TAURI__ at any point before the first call, and modules don't
// destructure the global at import time.

export function invoke(cmd, args) {
    return window.__TAURI__.core.invoke(cmd, args);
}

// Callable with or without `new`; always returns the real Tauri Channel.
export function Channel(...args) {
    return new window.__TAURI__.core.Channel(...args);
}

export function listen(...args) {
    return window.__TAURI__.event.listen(...args);
}
