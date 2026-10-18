//! Authorization policy tests: `contracts.md` §2's matrix, and SEC-009.
//!
//! ## What the central test asserts, and why it is not a restatement of the code
//!
//! `contracts.md` states the rule as a sentence -- "Cross-owner access returns `not_found`,
//! never `forbidden`, so identifiers are not confirmable" -- and marks most matrix cells with
//! a bare "no" without naming a code. So there is no table of expected answers to transcribe.
//!
//! The testable content is a property: **a caller who cannot see a resource must receive the
//! same answer whether that resource belongs to someone else or does not exist at all.**
//! That is asserted directly, over every actor and every operation, by comparing the two
//! decisions. It catches the class of bug where a policy answers `forbidden` for a resource
//! it found and `not_found` for one it did not -- precisely the oracle the contract forbids,
//! and precisely what a hand-written expected-answer table would have enshrined.

use gtt_control_plane::db::entities::UserStatus;
use gtt_control_plane::policy::{
    decide, suspended_user_may, Actor, Decision, ErrorCode, Operation, Target,
};
use uuid::Uuid;

/// Every matrix column, as an actor.
fn actors(
    owner: Uuid,
    stranger: Uuid,
    relay: Uuid,
    other_relay: Uuid,
) -> Vec<(&'static str, Actor)> {
    vec![
        (
            "owner",
            Actor::User {
                id: owner,
                status: UserStatus::Active,
            },
        ),
        (
            "other user",
            Actor::User {
                id: stranger,
                status: UserStatus::Active,
            },
        ),
        (
            "suspended stranger",
            Actor::User {
                id: stranger,
                status: UserStatus::Suspended,
            },
        ),
        ("relay", Actor::Relay { id: relay }),
        ("other relay", Actor::Relay { id: other_relay }),
        ("admin", Actor::Admin),
        ("anonymous", Actor::Anonymous),
        ("guest", Actor::Guest { tunnel_id: owner }),
    ]
}

/// Every matrix row.
fn operations() -> Vec<(&'static str, Operation)> {
    vec![
        ("create tunnel", Operation::CreateTunnel),
        ("list tunnels", Operation::ListTunnels),
        ("view tunnel", Operation::ViewTunnel),
        ("start tunnel", Operation::StartTunnel),
        ("stop tunnel", Operation::StopTunnel),
        ("terminate tunnel", Operation::TerminateTunnel),
        ("create invitation", Operation::CreateInvitation),
        ("revoke invitation", Operation::RevokeInvitation),
        ("list admissions", Operation::ListAdmissions),
        ("claim invitation", Operation::ClaimInvitation),
        ("report health", Operation::ReportHealth),
        ("report usage", Operation::ReportUsage),
        ("fetch relay state", Operation::FetchRelayState),
        ("suspend user", Operation::SuspendUser),
    ]
}

/// An admin is entitled to learn whether a resource exists, so it is the one actor exempt
/// from the indistinguishability requirement. Kept as a named predicate so a future change to
/// the rule has to change this list too.
fn exempt_from_indistinguishability(actor: Actor) -> bool {
    matches!(actor, Actor::Admin)
}

/// Which kind of target an operation actually carries.
///
/// The indistinguishability property is only meaningful over a pairing that can occur. An
/// earlier version fed `CreateTunnel` a `UserOwned` target -- which never happens, because
/// nothing resolves a tunnel id when creating one -- and the resulting mismatch looked like a
/// policy bug until the pairing was noticed. In practice the API layer resolves a tunnel id to
/// a user-owned target and a relay id to a relay-owned one; nothing else exists to pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetKind {
    /// A user's resource, or the absence of one.
    User,
    /// A relay's resource, or the absence of one.
    Relay,
    /// No resource at all.
    None,
}

fn target_kind(operation: Operation) -> TargetKind {
    match operation {
        Operation::CreateTunnel | Operation::ListTunnels | Operation::ClaimInvitation => {
            TargetKind::None
        }
        Operation::ReportHealth | Operation::ReportUsage | Operation::FetchRelayState => {
            TargetKind::Relay
        }
        _ => TargetKind::User,
    }
}

// ---------------------------------------------------------------------------
// The property
// ---------------------------------------------------------------------------

#[test]
fn a_strangers_resource_and_a_missing_one_give_the_same_answer() {
    let owner = Uuid::new_v4();
    let stranger = Uuid::new_v4();
    let relay = Uuid::new_v4();
    let other_relay = Uuid::new_v4();

    for (actor_name, actor) in actors(owner, stranger, relay, other_relay) {
        for (op_name, op) in operations() {
            if exempt_from_indistinguishability(actor) {
                continue;
            }
            match target_kind(op) {
                // Someone else's tunnel, and a tunnel that does not exist. The data layer
                // cannot tell these apart -- scoping the query by owner makes both come back
                // empty -- so neither may the policy.
                TargetKind::User => {
                    assert_eq!(
                        decide(
                            actor,
                            op,
                            Target::UserOwned {
                                owner: Some(&owner)
                            }
                        ),
                        decide(actor, op, Target::UserOwned { owner: None }),
                        "{actor_name} / {op_name}: a stranger's resource and a missing one are \
                         answered differently, which confirms whether the identifier exists"
                    );
                }
                TargetKind::Relay => {
                    assert_eq!(
                        decide(
                            actor,
                            op,
                            Target::RelayOwned {
                                owner: Some(&other_relay)
                            }
                        ),
                        decide(actor, op, Target::RelayOwned { owner: None }),
                        "{actor_name} / {op_name}: another relay's state and a missing one are \
                         answered differently, which confirms existence"
                    );
                }
                TargetKind::None => {
                    // Nothing whose existence could leak, so the property is vacuous here.
                    // `only_operations_without_a_resource_are_exempt` pins which operations
                    // fall in this bucket.
                }
            }
        }
    }
}

#[test]
fn cross_owner_access_is_never_forbidden() {
    // Named twice in the matrix ("no (`not_found`)") and generalised by the property above.
    // Asserted directly as well, so a failure names the rule that broke rather than a pair
    // of decisions that merely differ.
    let stranger = Actor::User {
        id: Uuid::new_v4(),
        status: UserStatus::Active,
    };
    let theirs = Target::UserOwned { owner: None };

    for (name, op) in operations() {
        if !op.names_a_resource() {
            continue;
        }
        assert_eq!(
            decide(stranger, op, theirs),
            Decision::Deny(ErrorCode::NotFound),
            "cross-owner {name} must be not_found, never forbidden"
        );
    }

    // And a suspended stranger gets the same answer: visibility is decided before
    // entitlement, so account state cannot be used as a side channel either.
    let suspended = Actor::User {
        id: Uuid::new_v4(),
        status: UserStatus::Suspended,
    };
    for (name, op) in operations() {
        if !op.names_a_resource() {
            continue;
        }
        assert_eq!(
            decide(suspended, op, theirs),
            Decision::Deny(ErrorCode::NotFound),
            "a suspended account must learn nothing about another user's {name}"
        );
    }
}

#[test]
fn an_admin_is_told_a_missing_resource_is_missing() {
    // The regression guard for the bug this module had: an admin was exempted from
    // visibility entirely, which meant asking about a tunnel that did not exist returned
    // "allowed". An admin is entitled to learn that a resource exists; not entitled to be told
    // a nonexistent resource is fine.
    for op in [
        Operation::ViewTunnel,
        Operation::StartTunnel,
        Operation::StopTunnel,
    ] {
        assert_eq!(
            decide(Actor::Admin, op, Target::UserOwned { owner: None }),
            Decision::Deny(ErrorCode::NotFound),
            "an admin asking about a tunnel that does not exist must be told not_found"
        );
        assert!(
            decide(
                Actor::Admin,
                op,
                Target::UserOwned {
                    owner: Some(&Uuid::new_v4())
                }
            )
            .is_allowed(),
            "an admin may act on a tunnel that does exist"
        );
    }
}

// ---------------------------------------------------------------------------
// The matrix, where the contract is explicit
// ---------------------------------------------------------------------------

#[test]
fn unauthenticated_is_answered_before_anything_else() {
    let owner = Uuid::new_v4();
    let relay = Uuid::new_v4();
    let other_relay = Uuid::new_v4();

    for (op_name, op) in operations() {
        let decisions = [
            decide(Actor::Anonymous, op, Target::None),
            decide(Actor::Anonymous, op, Target::UserOwned { owner: None }),
            decide(
                Actor::Anonymous,
                op,
                Target::UserOwned {
                    owner: Some(&owner),
                },
            ),
            decide(
                Actor::Anonymous,
                op,
                Target::RelayOwned {
                    owner: Some(&relay),
                },
            ),
            decide(
                Actor::Anonymous,
                op,
                Target::RelayOwned {
                    owner: Some(&other_relay),
                },
            ),
        ];
        for d in decisions {
            assert_eq!(
                d,
                Decision::Deny(ErrorCode::Unauthenticated),
                "anonymous / {op_name}: the answer must not depend on the target, so that \
                 probing identifiers reveals nothing"
            );
        }
    }
}

#[test]
fn a_guest_never_reaches_the_control_plane() {
    // ADR-007 G2: the guest has no account and no client; the relay enforces the admission
    // set. So the guest column of the matrix is "no" by construction for every operation that
    // needs credentials, and answered `unauthenticated` rather than `forbidden` -- because
    // answering `forbidden` would imply this layer is on the guest's path at all.
    //
    // The single exception is the claim row: the guest presenting an invitation secret *is*
    // the claimant, and it reaches that row with `Target::None`.
    let guest = Actor::Guest {
        tunnel_id: Uuid::new_v4(),
    };
    let owner = Uuid::new_v4();

    for (name, op) in operations() {
        if matches!(op, Operation::ClaimInvitation) {
            continue;
        }
        assert_eq!(
            decide(
                guest,
                op,
                Target::UserOwned {
                    owner: Some(&owner)
                }
            ),
            Decision::Deny(ErrorCode::Unauthenticated),
            "a guest must not be permitted to {name} through the control plane"
        );
    }

    assert!(
        decide(guest, Operation::ClaimInvitation, Target::None).is_allowed(),
        "a guest presenting an invitation secret is the claimant and must reach the claim row"
    );
}

#[test]
fn an_owner_may_manage_their_own_tunnels() {
    let owner = Uuid::new_v4();
    let me = Actor::User {
        id: owner,
        status: UserStatus::Active,
    };
    let mine = Target::UserOwned {
        owner: Some(&owner),
    };

    for (name, op) in [
        ("view", Operation::ViewTunnel),
        ("start", Operation::StartTunnel),
        ("stop", Operation::StopTunnel),
        ("create an invitation for", Operation::CreateInvitation),
        ("revoke an invitation on", Operation::RevokeInvitation),
        ("list admissions on", Operation::ListAdmissions),
    ] {
        assert!(
            decide(me, op, mine).is_allowed(),
            "the owner must be able to {name} it"
        );
    }
    assert!(decide(me, Operation::CreateTunnel, Target::None).is_allowed());
    assert!(decide(me, Operation::ListTunnels, Target::None).is_allowed());
}

#[test]
fn terminate_and_suspend_are_admin_only_even_for_the_owner() {
    // The matrix says "no" in the owner column too: terminate is an operator action,
    // distinct from the owner choosing to stop. Answered `forbidden` rather than
    // `not_found`, because a User actor gets that answer whether or not the tunnel exists --
    // which is what makes it safe to use.
    let owner = Uuid::new_v4();
    let me = Actor::User {
        id: owner,
        status: UserStatus::Active,
    };
    let mine = Target::UserOwned {
        owner: Some(&owner),
    };
    let missing = Target::UserOwned { owner: None };

    for target in [mine, missing] {
        assert_eq!(
            decide(me, Operation::TerminateTunnel, target),
            Decision::Deny(ErrorCode::Forbidden),
            "an owner must not be able to terminate, and the answer must not depend on \
             whether the tunnel exists"
        );
    }
    assert_eq!(
        decide(me, Operation::SuspendUser, mine),
        Decision::Deny(ErrorCode::Forbidden)
    );

    assert!(decide(Actor::Admin, Operation::TerminateTunnel, Target::None).is_allowed());
    assert!(decide(Actor::Admin, Operation::SuspendUser, Target::None).is_allowed());
}

#[test]
fn a_relay_may_only_report_about_itself() {
    let relay = Uuid::new_v4();
    let me = Actor::Relay { id: relay };
    let mine = Target::RelayOwned {
        owner: Some(&relay),
    };

    for (name, op) in [
        ("report health", Operation::ReportHealth),
        ("report usage", Operation::ReportUsage),
        ("fetch its own state", Operation::FetchRelayState),
    ] {
        assert!(
            decide(me, op, mine).is_allowed(),
            "a relay must be able to {name}"
        );
    }

    assert_eq!(
        decide(
            me,
            Operation::FetchRelayState,
            Target::RelayOwned { owner: None }
        ),
        Decision::Deny(ErrorCode::NotFound)
    );

    // A relay has no tunnels.
    assert_eq!(
        decide(me, Operation::CreateTunnel, Target::None),
        Decision::Deny(ErrorCode::Forbidden),
        "a relay must not be able to create a tunnel"
    );
}

#[test]
fn a_user_may_not_use_the_relay_row() {
    let owner = Uuid::new_v4();
    let relay = Uuid::new_v4();
    let me = Actor::User {
        id: owner,
        status: UserStatus::Active,
    };

    for (name, op) in [
        ("report health", Operation::ReportHealth),
        ("report usage", Operation::ReportUsage),
        ("fetch relay state", Operation::FetchRelayState),
    ] {
        assert_eq!(
            decide(
                me,
                op,
                Target::RelayOwned {
                    owner: Some(&relay)
                }
            ),
            Decision::Deny(ErrorCode::NotFound),
            "a user must not be able to {name} for a relay"
        );
    }
}

// ---------------------------------------------------------------------------
// Suspension
// ---------------------------------------------------------------------------

#[test]
fn a_suspended_account_cannot_create_or_start_but_can_still_clean_up() {
    // `data-model.md`: a suspended user cannot create or start tunnels or refresh tokens,
    // and their active tunnels are terminated on suspension. Refusing reads and stops as well
    // would leave a suspended owner unable to see or stop what they still have.
    let owner = Uuid::new_v4();
    let me = Actor::User {
        id: owner,
        status: UserStatus::Suspended,
    };
    let mine = Target::UserOwned {
        owner: Some(&owner),
    };

    assert_eq!(
        decide(me, Operation::CreateTunnel, Target::None),
        Decision::Deny(ErrorCode::Forbidden),
        "a suspended account must not create tunnels"
    );
    assert_eq!(
        decide(me, Operation::StartTunnel, mine),
        Decision::Deny(ErrorCode::Forbidden),
        "a suspended account must not start tunnels"
    );
    assert!(!suspended_user_may(Operation::CreateTunnel));
    assert!(!suspended_user_may(Operation::StartTunnel));

    for op in [
        Operation::ListTunnels,
        Operation::ViewTunnel,
        Operation::StopTunnel,
        Operation::RevokeInvitation,
        Operation::ListAdmissions,
    ] {
        assert!(
            suspended_user_may(op),
            "{op:?} should stay available when suspended"
        );
    }

    assert!(decide(me, Operation::ViewTunnel, mine).is_allowed());
    assert!(decide(me, Operation::StopTunnel, mine).is_allowed());
    assert!(decide(me, Operation::ListTunnels, Target::None).is_allowed());
}

// ---------------------------------------------------------------------------
// Claiming an invitation
// ---------------------------------------------------------------------------

#[test]
fn claiming_an_invitation_is_authorised_by_the_secret_not_the_account() {
    // The matrix marks this row "anyone with the secret", so the policy does not refuse it on
    // capability grounds. The secret is checked when presented, and the refusal there is
    // `invitation_invalid`, which says nothing about whether the invitation exists.
    //
    // Answered **without credentials**, because under ADR-007 G2 the claimant is anonymous:
    // the guest has no account and no client. Refusing anonymous callers before reaching this
    // row would make the route unreachable, which is what the first version of the policy did.
    for (name, actor) in [
        ("an anonymous claimant", Actor::Anonymous),
        (
            "a guest",
            Actor::Guest {
                tunnel_id: Uuid::new_v4(),
            },
        ),
        (
            "the owner",
            Actor::User {
                id: Uuid::new_v4(),
                status: UserStatus::Active,
            },
        ),
        (
            "a stranger",
            Actor::User {
                id: Uuid::new_v4(),
                status: UserStatus::Active,
            },
        ),
    ] {
        assert!(
            decide(actor, Operation::ClaimInvitation, Target::None).is_allowed(),
            "{name} must be able to attempt a claim; the secret decides"
        );
    }

    // The matrix marks the relay and admin columns "no" for this row.
    for (name, actor) in [
        ("a relay", Actor::Relay { id: Uuid::new_v4() }),
        ("an admin", Actor::Admin),
    ] {
        assert_eq!(
            decide(actor, Operation::ClaimInvitation, Target::None),
            Decision::Deny(ErrorCode::Forbidden),
            "the matrix marks the {name} column \"no\" for the claim row"
        );
    }

    // And a claim must not carry a target, or the route could be used to probe a resource.
    assert_eq!(
        decide(
            Actor::Anonymous,
            Operation::ClaimInvitation,
            Target::UserOwned { owner: None }
        ),
        Decision::Deny(ErrorCode::Forbidden),
        "a claim must not accept a target; the invitation is named by its secret"
    );
}

// ---------------------------------------------------------------------------
// Error codes
// ---------------------------------------------------------------------------

#[test]
fn error_codes_carry_the_statuses_the_contract_pairs_them_with() {
    // contracts.md section 3.
    for (code, status, wire) in [
        (ErrorCode::Unauthenticated, 401u16, "unauthenticated"),
        (ErrorCode::Forbidden, 403, "forbidden"),
        (ErrorCode::NotFound, 404, "not_found"),
        (ErrorCode::ValidationFailed, 400, "validation_failed"),
        (ErrorCode::Conflict, 409, "conflict"),
    ] {
        assert_eq!(code.http_status(), status, "{wire} has the wrong status");
        assert_eq!(code.as_str(), wire);
    }
}

#[test]
fn only_operations_without_a_resource_are_exempt() {
    // The exemption list is load-bearing: it is what decides which operations `forbidden` is
    // a complete answer for. If a new operation names a resource it must not land here.
    for (name, op) in operations() {
        let expected = !matches!(
            op,
            Operation::CreateTunnel | Operation::ListTunnels | Operation::ClaimInvitation
        );
        assert_eq!(
            op.names_a_resource(),
            expected,
            "{name}: names_a_resource disagrees with the documented set"
        );
    }
}
