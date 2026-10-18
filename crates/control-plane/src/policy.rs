//! Authorization policy: the central decision point for `contracts.md` §2's matrix.
//!
//! ## The rule this module implements, and why it is the right one
//!
//! `contracts.md` marks most non-allowed matrix cells with a bare "no" and names a code for
//! only two of them. The sentence immediately below the table resolves the rest:
//!
//! > Cross-owner access returns `not_found`, never `forbidden`, so identifiers are not
//! > confirmable.
//!
//! Read as a property rather than a per-cell instruction, it says something checkable:
//!
//! > **The response must not vary with the existence of a resource the caller cannot see.**
//!
//! That is what the tests in `tests/policy_unit.rs` assert, over the whole matrix, rather
//! than a list of expected answers per cell. Two consequences follow:
//!
//! * A **capability** failure — an actor class that can never perform this operation — is
//!   answered `forbidden` for *every* target. Whether the target exists or not, the answer
//!   is the same, so nothing is confirmed. Answering `not_found` for existing targets and
//!   `forbidden` for missing ones would be the leak.
//! * A **visibility** failure — a caller reaching for a resource that belongs to someone
//!   else — is answered `not_found` for *every* resource not owned by the caller. Again
//!   uniform, so nothing is confirmed.
//!
//! The ordering matters too. Visibility is decided before entitlement, so a **suspended**
//! user asking about someone else's tunnel gets `not_found` rather than `forbidden`, and
//! learns nothing about the tunnel; a suspended user asking about their own gets
//! `forbidden`, which is what the error model says a suspended account should see.
//!
//! ## One cell the document does not settle
//!
//! "Suspend user / terminate tunnel" is `no` for every actor except the admin, *including
//! the owner*. Terminate is an operator action, so a user calling it fails on capability
//! rather than on visibility, and the answer is `forbidden` whether or not the tunnel
//! exists. That is safe under the rule above -- the response does not vary with existence
//! -- but it is a reading, not a quotation, and it is recorded as an ADR candidate in
//! `research.md`.
//!
//! ## Guests
//!
//! ADR-007 chose G2: a guest connects to the relay with the stock game client and has **no
//! account and no guest client**. The guest column of the matrix is therefore all "no" by
//! construction, not by policy: the control plane is not in the guest's data path at all --
//! the relay enforces the live admission set. [`Actor::Guest`] exists so the column is
//! answered explicitly rather than left to a default.

use uuid::Uuid;

use crate::db::entities::UserStatus;

/// An error code from `contracts.md` §3.
///
/// The policy returns codes rather than HTTP statuses so that the HTTP mapping stays in one
/// place; [`ErrorCode::http_status`] is that place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    /// Missing, invalid, or expired credentials.
    Unauthenticated,
    /// Authenticated but not allowed. Also used for a suspended account.
    Forbidden,
    /// Resource absent **or not visible to the caller**. The two are indistinguishable on
    /// purpose.
    NotFound,
    /// Schema or domain validation failed.
    ValidationFailed,
    /// State conflict, e.g. starting a stopped tunnel.
    Conflict,
}

impl ErrorCode {
    /// The HTTP status `contracts.md` §3 pairs with this code.
    pub fn http_status(self) -> u16 {
        match self {
            Self::Unauthenticated => 401,
            Self::Forbidden => 403,
            Self::NotFound => 404,
            Self::ValidationFailed => 400,
            Self::Conflict => 409,
        }
    }

    /// The wire value, which is what an `ErrorResponse` carries.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unauthenticated => "unauthenticated",
            Self::Forbidden => "forbidden",
            Self::NotFound => "not_found",
            Self::ValidationFailed => "validation_failed",
            Self::Conflict => "conflict",
        }
    }
}

/// Who is making the request.
///
/// Deliberately not an enum with one variant per matrix column: the columns differ in kind,
/// not just in permission, and flattening them would hide that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    /// No credentials at all.
    Anonymous,
    /// A `users` row that authenticated.
    User {
        /// The caller's own id.
        id: Uuid,
        /// Account state. A suspended user is authenticated but not allowed, which is what
        /// `forbidden` means in the error model.
        status: UserStatus,
    },
    /// The separate admin identity (ADR-019). Never a role on `users`, so an admin session
    /// can never hold a tunnel and a user session can never carry admin capability.
    Admin,
    /// A relay presenting its own credential.
    Relay {
        /// Which relay. A relay may only ever act as itself.
        id: Uuid,
    },
    /// An admitted guest. Carries the tunnel it was admitted to, though under G2 it never
    /// reaches this layer -- see the module docs.
    Guest {
        /// The tunnel whose allocation the guest is using.
        tunnel_id: Uuid,
    },
}

/// Which resource a request is about, and who owns it.
///
/// This is the whole of the caller's knowledge about the resource. The policy is not given
/// the row: it is given the answer to "is this yours", which the data layer already had to
/// establish in order to scope the query at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target<'a> {
    /// No specific resource. Creating a tunnel, or listing one's own tunnels: there is
    /// nothing whose existence could be confirmed.
    None,
    /// A resource belonging to a user. `None` means "no such resource **visible to you**" --
    /// absent and not-yours are deliberately the same value.
    UserOwned {
        /// The owner, if the resource is visible to the caller as theirs.
        owner: Option<&'a Uuid>,
    },
    /// A resource belonging to a relay.
    RelayOwned {
        /// The relay that owns it, if the caller is that relay.
        owner: Option<&'a Uuid>,
    },
}

/// What the caller is trying to do.
///
/// One variant per matrix row, except where a row's members behave differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    /// Create a tunnel.
    CreateTunnel,
    /// List the caller's own tunnels.
    ListTunnels,
    /// Read one tunnel.
    ViewTunnel,
    /// Move a tunnel to `active`.
    StartTunnel,
    /// Stop a tunnel the owner chose to stop.
    StopTunnel,
    /// Terminate a tunnel. An operator action: the matrix says "no" for the owner too.
    TerminateTunnel,
    /// Create an invitation against a tunnel.
    CreateInvitation,
    /// Revoke an invitation.
    RevokeInvitation,
    /// List the live admission set for a tunnel.
    ListAdmissions,
    /// Claim an invitation with a secret. Anyone may attempt this; the secret is the
    /// authority, which is why it is the one row the matrix marks "anyone with the secret".
    ClaimInvitation,
    /// Report relay health.
    ReportHealth,
    /// Report a usage interval for a tunnel on a relay.
    ReportUsage,
    /// Read a relay's state.
    FetchRelayState,
    /// Suspend a user. Admin only.
    SuspendUser,
}

impl Operation {
    /// Whether this operation names a resource, and so could confirm its existence.
    ///
    /// This is the predicate the no-leak property is stated over: a refusal of an operation
    /// that names a resource must not vary with whether that resource exists. The three that
    /// do not name one -- create, list-own, and claim-by-secret -- have no existence to leak,
    /// which is why `forbidden` is a complete and safe answer for them.
    ///
    /// Public because the tests assert the property over it, and a property only the
    /// implementation can talk about is not a property anyone is checking.
    pub fn names_a_resource(self) -> bool {
        !matches!(
            self,
            Self::CreateTunnel | Self::ListTunnels | Self::ClaimInvitation
        )
    }
}

/// The outcome of a policy decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Permitted.
    Allow,
    /// Refused, with the code to return.
    Deny(ErrorCode),
}

impl Decision {
    /// The error code if refused, `None` if allowed.
    pub fn error(self) -> Option<ErrorCode> {
        match self {
            Self::Allow => None,
            Self::Deny(code) => Some(code),
        }
    }

    /// True when permitted.
    pub fn is_allowed(self) -> bool {
        matches!(self, Self::Allow)
    }
}

/// Decide whether `actor` may perform `operation` against `target`.
///
/// Checks run in this order, and the order is load-bearing:
///
/// 0. **A claim carries no target.** An invitation is identified by its secret, never by an
///    id, so requiring `Target::None` stops the claim route being used to probe resources.
/// 1. **Credentials**, before visibility. An anonymous caller learns the same thing whatever
///    they ask for; folding this into the dispatch lets existence leak through the choice of
///    code.
/// 2. **Visibility**, before entitlement, so a suspended account learns nothing about
///    resources that are not its own.
/// 3. **Entitlement** — a suspended account.
/// 4. **Capability**, dispatched by actor kind, so every matrix column is visible in one
///    place and a new actor variant fails to compile rather than falling through.
///
/// There is deliberately **no early return for unauthenticated callers.** Under ADR-007 G2
/// the guest has no account, so the entity presenting an invitation secret is anonymous by
/// design; refusing anonymous callers before reaching the claim row would make the route
/// unreachable. An earlier version did exactly that and a test caught it.
pub fn decide(actor: Actor, operation: Operation, target: Target<'_>) -> Decision {
    let claiming = matches!(operation, Operation::ClaimInvitation);

    // 0. A claim must not carry a target.
    if claiming && !matches!(target, Target::None) {
        return Decision::Deny(ErrorCode::Forbidden);
    }

    // 1. Credentials, before visibility.
    //
    //    Order matters and getting it wrong is a leak in its own right: an earlier version
    //    folded this into the dispatch below, so an anonymous caller fell through to the
    //    visibility check and got `not_found` for one target and `unauthenticated` for
    //    another -- which is precisely the oracle this module exists to prevent. A caller with
    //    no credentials learns the same thing whatever they ask for.
    //
    //    The claim row is exempt, because under ADR-007 G2 the claimant is anonymous.
    if !claiming && matches!(actor, Actor::Anonymous | Actor::Guest { .. }) {
        return Decision::Deny(ErrorCode::Unauthenticated);
    }

    // 2. Visibility, before entitlement, so a suspended account learns nothing about
    //    resources that are not its own. A claim is authorised by the secret rather than by
    //    ownership of anything, so it has no visibility to decide.
    if !claiming {
        if let Some(code) = visibility_failure(actor, target) {
            return Decision::Deny(code);
        }
    }

    // 3. Entitlement. A suspended account is authenticated but not allowed, which is exactly
    //    what `forbidden` means in the error model.
    if let Actor::User { status, .. } = actor {
        if status == UserStatus::Suspended && !suspended_user_may(operation) {
            return Decision::Deny(ErrorCode::Forbidden);
        }
    }

    // 4. Capability.
    match actor {
        Actor::Admin => match operation {
            Operation::TerminateTunnel
            | Operation::SuspendUser
            | Operation::CreateTunnel
            | Operation::ListTunnels
            | Operation::ViewTunnel
            | Operation::StartTunnel
            | Operation::StopTunnel
            | Operation::CreateInvitation
            | Operation::RevokeInvitation
            | Operation::ListAdmissions => Decision::Allow,
            // An admin has no relay of its own, and the matrix marks the claim row "no" for
            // it. `forbidden` rather than `unauthenticated`: the credential is fine, the
            // capability is not.
            Operation::ReportHealth
            | Operation::ReportUsage
            | Operation::FetchRelayState
            | Operation::ClaimInvitation => Decision::Deny(ErrorCode::Forbidden),
        },

        Actor::User { .. } => match operation {
            Operation::CreateTunnel
            | Operation::ListTunnels
            | Operation::ViewTunnel
            | Operation::StartTunnel
            | Operation::StopTunnel
            | Operation::CreateInvitation
            | Operation::RevokeInvitation
            | Operation::ListAdmissions => Decision::Allow,
            // The matrix marks the claim row "anyone with the secret", so the secret decides
            // and the policy does not refuse on capability grounds. The refusal, if the
            // secret is wrong, is `invitation_invalid`, which says nothing about whether the
            // invitation exists.
            Operation::ClaimInvitation => Decision::Allow,
            // The relay row and the admin-only row: both "no" for a user.
            Operation::ReportHealth
            | Operation::ReportUsage
            | Operation::FetchRelayState
            | Operation::TerminateTunnel
            | Operation::SuspendUser => Decision::Deny(ErrorCode::Forbidden),
        },

        Actor::Relay { .. } => match operation {
            Operation::ReportHealth | Operation::ReportUsage | Operation::FetchRelayState => {
                Decision::Allow
            }
            // A relay has no tunnels and no invitations, and the matrix marks the claim row
            // "no" for it. Terminate and SuspendUser are operator actions on someone else's
            // resources.
            _ => Decision::Deny(ErrorCode::Forbidden),
        },

        // No credentials, and under ADR-007 G2 a guest has none by design. Reached only for
        // the claim row, because the credential check above returns first for everything
        // else. The second arm is unreachable in practice; stated rather than
        // `unreachable!()` because that macro is in the denied `panic` lint group.
        Actor::Anonymous | Actor::Guest { .. } => match operation {
            Operation::ClaimInvitation => Decision::Allow,
            _ => Decision::Deny(ErrorCode::Unauthenticated),
        },
    }
}

/// `not_found` when the caller cannot see the resource, `None` when visibility is not the
/// obstacle.
///
/// Two arms per target kind, and the `None` arm is the important one. `Target::UserOwned
/// { owner: None }` is **also** how a nonexistent resource resolves, because the data layer
/// cannot tell them apart any more than the policy can -- scoping the query by owner is what
/// made both come back empty. So:
///
/// * `owner: None` answers `not_found` for every authenticated actor that names a resource.
/// * `owner: Some(x)` answers `not_found` for a `User` actor when `x` is not them, and for a
///   `Relay` actor, because neither has any business with a user's resource. An `Admin` sees
///   it: the matrix says admins act "via admin API", and an admin is entitled to know a
///   resource exists.
///
/// The pair therefore returns the *same* answer for "belongs to someone else" and "does not
/// exist" for every actor except an admin, which is the property `contracts.md` requires.
/// An earlier version exempted admins from visibility altogether, which meant an admin
/// asking about a tunnel that did not exist was told it was allowed -- the same leak, aimed
/// the other way.
fn visibility_failure(actor: Actor, target: Target<'_>) -> Option<ErrorCode> {
    match target {
        Target::None => None,

        Target::UserOwned { owner } => match owner {
            // Absent, or not visible. Same answer either way, by construction.
            None => Some(ErrorCode::NotFound),
            Some(id) => match actor {
                // A relay has no tunnels.
                Actor::Relay { .. } => Some(ErrorCode::NotFound),
                // Someone else's tunnel.
                Actor::User { id: mine, .. } if mine != *id => Some(ErrorCode::NotFound),
                // Their own, or an admin's to act on.
                _ => None,
            },
        },

        Target::RelayOwned { owner } => match owner {
            // Absent, or not this relay's.
            None => Some(ErrorCode::NotFound),
            Some(id) => match actor {
                Actor::Relay { id: mine } if mine == *id => None,
                _ => Some(ErrorCode::NotFound),
            },
        },
    }
}

/// Whether a suspended account may still perform `operation`.
///
/// The rule from `data-model.md`: a suspended user cannot **create or start** tunnels or
/// refresh tokens, and their existing active tunnels are terminated on suspension. So
/// creation and starting are refused. Everything that lets an owner *inspect or clean up*
/// what they already have stays available, because refusing those would leave a suspended
/// user unable to see or stop their own tunnels -- and a control plane you cannot stop is
/// worse than one you cannot start.
///
/// `ClaimInvitation` is included because it is authorised by the secret rather than by the
/// account: whoever presents the secret is the guest, and a guest has no suspended state.
pub fn suspended_user_may(operation: Operation) -> bool {
    matches!(
        operation,
        Operation::ListTunnels
            | Operation::ViewTunnel
            | Operation::StopTunnel
            | Operation::RevokeInvitation
            | Operation::ListAdmissions
            | Operation::ClaimInvitation
    )
}
