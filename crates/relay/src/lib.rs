//! Relay service: allocation validation, public listeners, flow tables, and limits.
//!
//! Must never open a connection to a destination chosen by a client (SEC-005, ADR-003). Implemented by T-060.
