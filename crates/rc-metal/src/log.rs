//! Bevy's log macros (`info!`, `warn!`, `error!`, `debug!`, `trace!`): lines on stdout / stderr.

#[macro_export]
macro_rules! info { ($($t:tt)*) => { println!($($t)*) } }
#[macro_export]
macro_rules! warn { ($($t:tt)*) => { eprintln!("WARN {}", format!($($t)*)) } }
#[macro_export]
macro_rules! error { ($($t:tt)*) => { eprintln!("ERROR {}", format!($($t)*)) } }
#[macro_export]
macro_rules! debug { ($($t:tt)*) => { if false { println!($($t)*) } } }
#[macro_export]
macro_rules! trace { ($($t:tt)*) => { if false { println!($($t)*) } } }
