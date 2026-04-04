#![cfg(test)]
extern crate std;

use crate::{types::ServiceStatus, Registry, RegistryClient};
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token, Address, Bytes, BytesN, Env,
};

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

fn create_token<'a>(
    e: &Env,
    admin: &Address,
) -> (token::Client<'a>, token::StellarAssetClient<'a>) {
    let sac = e.register_stellar_asset_contract_v2(admin.clone());
    (
        token::Client::new(e, &sac.address()),
        token::StellarAssetClient::new(e, &sac.address()),
    )
}

struct Setup<'a> {
    env: Env,
    admin: Address,
    token: token::Client<'a>,
    token_sac: token::StellarAssetClient<'a>,
    registry: RegistryClient<'a>,
}

impl<'a> Setup<'a> {
    fn new() -> Self {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().with_mut(|li| {
            li.timestamp = 1_000_000;
        });

        let admin = Address::generate(&env);
        let token_admin = Address::generate(&env);
        let (token, token_sac) = create_token(&env, &token_admin);

        let min_bond: i128 = 10_0000000; // 10 tokens
        let cooldown: u64 = 604_800; // 7 days

        let contract_id = env.register(Registry, (&admin, &token.address, &min_bond, &cooldown));
        let registry = RegistryClient::new(&env, &contract_id);

        // Fund admin for reward seeding
        token_sac.mint(&admin, &10_000_0000000);

        Setup {
            env,
            admin,
            token,
            token_sac,
            registry,
        }
    }

    /// Create a funded service address.
    fn service(&self) -> Address {
        let addr = Address::generate(&self.env);
        self.token_sac.mint(&addr, &1_000_0000000); // 1 000 tokens
        addr
    }

    fn advance(&self, seconds: u64) {
        self.env.ledger().with_mut(|li| {
            li.timestamp += seconds;
        });
    }

    fn meta1(&self) -> Bytes {
        Bytes::from_slice(&self.env, b"service-metadata-1")
    }

    fn meta2(&self) -> Bytes {
        Bytes::from_slice(&self.env, b"service-metadata-2")
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

#[test]
fn test_register() {
    let t = Setup::new();
    let svc = t.service();

    t.registry.register(&svc, &t.meta1(), &50_0000000);

    let info = t.registry.get(&svc);
    assert_eq!(info.bond, 50_0000000);
    assert_eq!(info.reputation, 10000); // default 100% (optimistic)
    assert_eq!(info.status, ServiceStatus::Active);
    assert_eq!(info.pending_unstake, 0);
    assert_eq!(info.registered_at, 1_000_000);
    // Token balances
    assert_eq!(t.token.balance(&svc), 950_0000000); // 1000 - 50
    assert_eq!(t.registry.total_bonds(), 50_0000000);
}

#[test]
#[should_panic]
fn test_register_insufficient_bond() {
    let t = Setup::new();
    let svc = t.service();
    t.registry.register(&svc, &t.meta1(), &5_0000000); // 5 < 10 min
}

#[test]
#[should_panic]
fn test_register_already_registered() {
    let t = Setup::new();
    let svc = t.service();
    t.registry.register(&svc, &t.meta1(), &50_0000000);
    t.registry.register(&svc, &t.meta1(), &50_0000000);
}

#[test]
#[should_panic]
fn test_register_invalid_amount() {
    let t = Setup::new();
    let svc = t.service();
    t.registry.register(&svc, &t.meta1(), &0);
}

// ---------------------------------------------------------------------------
// Get
// ---------------------------------------------------------------------------

#[test]
#[should_panic]
fn test_get_not_registered() {
    let t = Setup::new();
    let svc = Address::generate(&t.env);
    t.registry.get(&svc);
}

// ---------------------------------------------------------------------------
// Deregister
// ---------------------------------------------------------------------------

#[test]
fn test_deregister() {
    let t = Setup::new();
    let svc = t.service();
    t.registry.register(&svc, &t.meta1(), &50_0000000);

    t.registry.deregister(&svc);

    let info = t.registry.get(&svc);
    assert_eq!(info.status, ServiceStatus::Unbonding);
    assert_eq!(info.pending_unstake, 50_0000000);
    assert_eq!(info.unstake_time, 1_000_000);
}

#[test]
#[should_panic]
fn test_deregister_not_registered() {
    let t = Setup::new();
    let svc = Address::generate(&t.env);
    t.registry.deregister(&svc);
}

#[test]
#[should_panic]
fn test_deregister_not_active() {
    let t = Setup::new();
    let svc = t.service();
    t.registry.register(&svc, &t.meta1(), &50_0000000);
    t.registry.deregister(&svc);
    t.registry.deregister(&svc); // already unbonding
}

// ---------------------------------------------------------------------------
// Update metadata
// ---------------------------------------------------------------------------

#[test]
fn test_update_metadata() {
    let t = Setup::new();
    let svc = t.service();
    t.registry.register(&svc, &t.meta1(), &50_0000000);

    t.registry.update_metadata(&svc, &t.meta2());

    let info = t.registry.get(&svc);
    let expected: BytesN<32> = t.env.crypto().sha256(&t.meta2()).into();
    assert_eq!(info.metadata_hash, expected);
}

// ---------------------------------------------------------------------------
// Stake
// ---------------------------------------------------------------------------

#[test]
fn test_stake() {
    let t = Setup::new();
    let svc = t.service();
    t.registry.register(&svc, &t.meta1(), &50_0000000);

    t.registry.stake(&svc, &30_0000000);

    let info = t.registry.get(&svc);
    assert_eq!(info.bond, 80_0000000);
    assert_eq!(t.token.balance(&svc), 920_0000000); // 1000 - 50 - 30
    assert_eq!(t.registry.total_bonds(), 80_0000000);
}

#[test]
#[should_panic]
fn test_stake_invalid_amount() {
    let t = Setup::new();
    let svc = t.service();
    t.registry.register(&svc, &t.meta1(), &50_0000000);
    t.registry.stake(&svc, &0);
}

#[test]
#[should_panic]
fn test_stake_not_active() {
    let t = Setup::new();
    let svc = t.service();
    t.registry.register(&svc, &t.meta1(), &50_0000000);
    t.registry.deregister(&svc);
    t.registry.stake(&svc, &10_0000000);
}

// ---------------------------------------------------------------------------
// Unstake
// ---------------------------------------------------------------------------

#[test]
fn test_unstake() {
    let t = Setup::new();
    let svc = t.service();
    t.registry.register(&svc, &t.meta1(), &50_0000000);

    t.registry.unstake(&svc, &20_0000000);

    let info = t.registry.get(&svc);
    assert_eq!(info.pending_unstake, 20_0000000);
    assert_eq!(info.unstake_time, 1_000_000);
    // Bond unchanged during cooldown
    assert_eq!(info.bond, 50_0000000);
}

#[test]
#[should_panic]
fn test_unstake_below_minimum() {
    let t = Setup::new();
    let svc = t.service();
    t.registry.register(&svc, &t.meta1(), &50_0000000);
    // Unstaking 45 leaves 5 < 10 min_bond
    t.registry.unstake(&svc, &45_0000000);
}

#[test]
#[should_panic]
fn test_unstake_pending_exists() {
    let t = Setup::new();
    let svc = t.service();
    t.registry.register(&svc, &t.meta1(), &50_0000000);
    t.registry.unstake(&svc, &10_0000000);
    t.registry.unstake(&svc, &10_0000000); // already pending
}

// ---------------------------------------------------------------------------
// Withdraw
// ---------------------------------------------------------------------------

#[test]
fn test_withdraw_after_cooldown() {
    let t = Setup::new();
    let svc = t.service();
    t.registry.register(&svc, &t.meta1(), &50_0000000);

    t.registry.unstake(&svc, &20_0000000);
    t.advance(604_800); // 7 days

    t.registry.withdraw(&svc);

    let info = t.registry.get(&svc);
    assert_eq!(info.bond, 30_0000000); // 50 - 20
    assert_eq!(info.pending_unstake, 0);
    assert_eq!(t.token.balance(&svc), 970_0000000); // 1000 - 50 + 20
    assert_eq!(t.registry.total_bonds(), 30_0000000);
}

#[test]
#[should_panic]
fn test_withdraw_before_cooldown() {
    let t = Setup::new();
    let svc = t.service();
    t.registry.register(&svc, &t.meta1(), &50_0000000);
    t.registry.unstake(&svc, &20_0000000);
    t.advance(604_799); // 1 second short
    t.registry.withdraw(&svc);
}

#[test]
#[should_panic]
fn test_withdraw_no_pending() {
    let t = Setup::new();
    let svc = t.service();
    t.registry.register(&svc, &t.meta1(), &50_0000000);
    t.registry.withdraw(&svc); // nothing pending
}

// ---------------------------------------------------------------------------
// Deregister + withdraw full lifecycle
// ---------------------------------------------------------------------------

#[test]
fn test_deregister_and_withdraw() {
    let t = Setup::new();
    let svc = t.service();
    t.registry.register(&svc, &t.meta1(), &50_0000000);

    t.registry.deregister(&svc);
    t.advance(604_800);
    t.registry.withdraw(&svc);

    // All tokens returned
    assert_eq!(t.token.balance(&svc), 1_000_0000000);
    assert_eq!(t.registry.total_bonds(), 0);
}

#[test]
#[should_panic]
fn test_get_after_full_withdraw() {
    let t = Setup::new();
    let svc = t.service();
    t.registry.register(&svc, &t.meta1(), &50_0000000);
    t.registry.deregister(&svc);
    t.advance(604_800);
    t.registry.withdraw(&svc);
    // Service removed
    t.registry.get(&svc);
}

// ---------------------------------------------------------------------------
// Slash
// ---------------------------------------------------------------------------

#[test]
fn test_slash_by_admin() {
    let t = Setup::new();
    let svc = t.service();
    let reporter = Address::generate(&t.env);
    t.registry.register(&svc, &t.meta1(), &100_0000000);

    t.registry.slash(&t.admin, &svc, &20_0000000, &reporter);

    let info = t.registry.get(&svc);
    assert_eq!(info.bond, 80_0000000); // 100 - 20
    assert_eq!(t.registry.total_bonds(), 80_0000000);
    // Reporter gets 30% = 6
    assert_eq!(t.token.balance(&reporter), 6_0000000);
}

#[test]
fn test_slash_with_treasury() {
    let t = Setup::new();
    let svc = t.service();
    let reporter = Address::generate(&t.env);
    let treasury = Address::generate(&t.env);

    t.registry.set_treasury(&treasury);
    t.registry.register(&svc, &t.meta1(), &100_0000000);

    t.registry.slash(&t.admin, &svc, &100_0000000, &reporter);

    // 30% reporter = 30, 20% treasury = 20, 50% pool stays in contract
    assert_eq!(t.token.balance(&reporter), 30_0000000);
    assert_eq!(t.token.balance(&treasury), 20_0000000);
}

#[test]
#[should_panic]
fn test_slash_unauthorized() {
    let t = Setup::new();
    let svc = t.service();
    let attacker = Address::generate(&t.env);
    let reporter = Address::generate(&t.env);
    t.registry.register(&svc, &t.meta1(), &50_0000000);
    t.registry.slash(&attacker, &svc, &10_0000000, &reporter);
}

#[test]
fn test_slash_by_dispute_resolver() {
    let t = Setup::new();
    let resolver = Address::generate(&t.env);
    t.registry.set_dispute_resolver(&resolver);

    let svc = t.service();
    let reporter = Address::generate(&t.env);
    t.registry.register(&svc, &t.meta1(), &50_0000000);

    t.registry.slash(&resolver, &svc, &10_0000000, &reporter);

    let info = t.registry.get(&svc);
    assert_eq!(info.bond, 40_0000000);
}

#[test]
fn test_slash_depletes_bond() {
    let t = Setup::new();
    let svc = t.service();
    let reporter = Address::generate(&t.env);
    t.registry.register(&svc, &t.meta1(), &50_0000000);

    t.registry.slash(&t.admin, &svc, &50_0000000, &reporter);

    assert_eq!(t.registry.total_bonds(), 0);
}

#[test]
#[should_panic]
fn test_slash_depleted_removes_service() {
    let t = Setup::new();
    let svc = t.service();
    let reporter = Address::generate(&t.env);
    t.registry.register(&svc, &t.meta1(), &50_0000000);
    t.registry.slash(&t.admin, &svc, &50_0000000, &reporter);
    // Service removed
    t.registry.get(&svc);
}

#[test]
fn test_slash_caps_to_bond() {
    let t = Setup::new();
    let svc = t.service();
    let reporter = Address::generate(&t.env);
    t.registry.register(&svc, &t.meta1(), &50_0000000);

    // Slash more than bond — capped to 50
    t.registry.slash(&t.admin, &svc, &999_0000000, &reporter);

    // Reporter gets 30% of 50 = 15
    assert_eq!(t.token.balance(&reporter), 15_0000000);
    assert_eq!(t.registry.total_bonds(), 0);
}

#[test]
fn test_slash_during_unbonding() {
    let t = Setup::new();
    let svc = t.service();
    let reporter = Address::generate(&t.env);
    t.registry.register(&svc, &t.meta1(), &100_0000000);

    t.registry.deregister(&svc);
    // Bond still slashable during cooldown
    t.registry.slash(&t.admin, &svc, &40_0000000, &reporter);

    let info = t.registry.get(&svc);
    assert_eq!(info.bond, 60_0000000);
    // pending_unstake capped to remaining bond
    assert_eq!(info.pending_unstake, 60_0000000);
}

// ---------------------------------------------------------------------------
// Rewards (Synthetix reward-per-token)
// ---------------------------------------------------------------------------

#[test]
fn test_notify_and_claim() {
    let t = Setup::new();
    let svc = t.service();
    t.registry.register(&svc, &t.meta1(), &100_0000000);

    // 100 tokens over 1000 seconds
    t.registry.notify_reward_amount(&100_0000000, &1000);

    t.advance(1000);

    let claimed = t.registry.claim_rewards(&svc);
    assert_eq!(claimed, 100_0000000); // sole staker gets all
}

#[test]
fn test_rewards_partial_duration() {
    let t = Setup::new();
    let svc = t.service();
    t.registry.register(&svc, &t.meta1(), &100_0000000);

    t.registry.notify_reward_amount(&100_0000000, &1000);

    t.advance(500); // half

    let claimed = t.registry.claim_rewards(&svc);
    assert_eq!(claimed, 50_0000000);
}

#[test]
fn test_rewards_two_equal_stakers() {
    let t = Setup::new();
    let svc1 = t.service();
    let svc2 = t.service();

    t.registry.register(&svc1, &t.meta1(), &100_0000000);
    t.registry.register(&svc2, &t.meta2(), &100_0000000);

    t.registry.notify_reward_amount(&100_0000000, &1000);

    t.advance(1000);

    let c1 = t.registry.claim_rewards(&svc1);
    let c2 = t.registry.claim_rewards(&svc2);
    assert_eq!(c1, 50_0000000);
    assert_eq!(c2, 50_0000000);
}

#[test]
fn test_rewards_proportional_to_bond() {
    let t = Setup::new();
    let svc1 = t.service();
    let svc2 = t.service();

    // 75:25 ratio
    t.registry.register(&svc1, &t.meta1(), &75_0000000);
    t.registry.register(&svc2, &t.meta2(), &25_0000000);

    t.registry.notify_reward_amount(&100_0000000, &1000);

    t.advance(1000);

    let c1 = t.registry.claim_rewards(&svc1);
    let c2 = t.registry.claim_rewards(&svc2);
    assert_eq!(c1, 75_0000000);
    assert_eq!(c2, 25_0000000);
}

#[test]
fn test_rewards_late_staker() {
    let t = Setup::new();
    let svc1 = t.service();
    let svc2 = t.service();

    t.registry.register(&svc1, &t.meta1(), &100_0000000);

    // 100 tokens over 1000s
    t.registry.notify_reward_amount(&100_0000000, &1000);

    // svc1 earns alone for 500s
    t.advance(500);

    // svc2 joins with equal stake at midpoint
    t.registry.register(&svc2, &t.meta2(), &100_0000000);

    // Remaining 500s split 50/50
    t.advance(500);

    let c1 = t.registry.claim_rewards(&svc1);
    let c2 = t.registry.claim_rewards(&svc2);

    // svc1: 50 (alone) + 25 (shared) = 75
    // svc2: 25 (shared)
    assert_eq!(c1, 75_0000000);
    assert_eq!(c2, 25_0000000);
}

#[test]
fn test_claim_no_rewards() {
    let t = Setup::new();
    let svc = t.service();
    t.registry.register(&svc, &t.meta1(), &50_0000000);

    let claimed = t.registry.claim_rewards(&svc);
    assert_eq!(claimed, 0);
}

// ---------------------------------------------------------------------------
// Admin
// ---------------------------------------------------------------------------

#[test]
fn test_set_admin() {
    let t = Setup::new();
    let new_admin = Address::generate(&t.env);
    t.registry.set_admin(&new_admin);
    assert_eq!(t.registry.admin(), new_admin);
}

#[test]
fn test_read_only_helpers() {
    let t = Setup::new();
    assert_eq!(t.registry.admin(), t.admin);
    assert_eq!(t.registry.token(), t.token.address);
    assert_eq!(t.registry.min_bond(), 10_0000000);
    assert_eq!(t.registry.total_bonds(), 0);
}

// ---------------------------------------------------------------------------
// Reputation
// ---------------------------------------------------------------------------

#[test]
fn test_reputation_decreases_on_slash() {
    let t = Setup::new();
    let svc = t.service();
    let reporter = Address::generate(&t.env);
    t.registry.register(&svc, &t.meta1(), &100_0000000);

    t.registry.slash(&t.admin, &svc, &10_0000000, &reporter);

    let info = t.registry.get(&svc);
    // 10000 - 1000 (SLASH_REPUTATION_PENALTY) = 9000
    assert_eq!(info.reputation, 9000);
}

#[test]
fn test_reputation_multiple_slashes() {
    let t = Setup::new();
    let svc = t.service();
    let reporter = Address::generate(&t.env);
    t.registry.register(&svc, &t.meta1(), &100_0000000);

    t.registry.slash(&t.admin, &svc, &5_0000000, &reporter);
    t.registry.slash(&t.admin, &svc, &5_0000000, &reporter);

    let info = t.registry.get(&svc);
    // 10000 - 1000 - 1000 = 8000
    assert_eq!(info.reputation, 8000);
}

#[test]
fn test_reputation_recovers_over_time() {
    let t = Setup::new();
    let svc = t.service();
    let reporter = Address::generate(&t.env);
    t.registry.register(&svc, &t.meta1(), &100_0000000);

    t.registry.slash(&t.admin, &svc, &10_0000000, &reporter);

    // After slash: stored reputation = 9000
    // Advance 10 days → recovery = 10 * 50 = 500 bps
    t.advance(10 * 86400);

    let info = t.registry.get(&svc);
    // 9000 + 500 = 9500
    assert_eq!(info.reputation, 9500);
}

#[test]
fn test_reputation_caps_at_max() {
    let t = Setup::new();
    let svc = t.service();
    let reporter = Address::generate(&t.env);
    t.registry.register(&svc, &t.meta1(), &100_0000000);

    t.registry.slash(&t.admin, &svc, &10_0000000, &reporter);

    // Advance 30 days → recovery = 30 * 50 = 1500 bps
    // 9000 + 1500 = 10500 → capped at 10000
    t.advance(30 * 86400);

    let info = t.registry.get(&svc);
    assert_eq!(info.reputation, 10000);
}

#[test]
fn test_reputation_floors_at_zero() {
    let t = Setup::new();
    let svc = t.service();
    let reporter = Address::generate(&t.env);
    // Large bond so we can slash many times
    t.registry.register(&svc, &t.meta1(), &500_0000000);

    // Slash 10 times → penalty = 10 * 1000 = 10000 → reputation = 0
    for _ in 0..10 {
        t.registry.slash(&t.admin, &svc, &5_0000000, &reporter);
    }

    let info = t.registry.get(&svc);
    assert_eq!(info.reputation, 0);
}

// ---------------------------------------------------------------------------
// Accrued rewards on full slash (C1)
// ---------------------------------------------------------------------------

#[test]
fn test_accrued_rewards_paid_on_full_slash() {
    let t = Setup::new();
    let svc = t.service();
    let reporter = Address::generate(&t.env);
    t.registry.register(&svc, &t.meta1(), &100_0000000);

    // Seed rewards: 100 tokens over 1000s
    t.registry.notify_reward_amount(&100_0000000, &1000);
    t.advance(500); // accrue 50 tokens

    let balance_before = t.token.balance(&svc);

    // Slash entire bond — accrued rewards should still be paid out
    t.registry.slash(&t.admin, &svc, &100_0000000, &reporter);

    let balance_after = t.token.balance(&svc);
    // Service should receive its accrued rewards (~50 tokens)
    assert_eq!(balance_after - balance_before, 50_0000000);
}

// ---------------------------------------------------------------------------
// Accrued rewards on full deregister + withdraw (C2)
// ---------------------------------------------------------------------------

#[test]
fn test_accrued_rewards_paid_on_full_withdraw() {
    let t = Setup::new();
    let svc = t.service();
    t.registry.register(&svc, &t.meta1(), &100_0000000);

    // Seed rewards: 100 tokens over 1000s
    t.registry.notify_reward_amount(&100_0000000, &1000);
    t.advance(1000); // accrue all 100 tokens

    t.registry.deregister(&svc);
    t.advance(604_800); // cooldown

    let balance_before = t.token.balance(&svc);
    t.registry.withdraw(&svc);
    let balance_after = t.token.balance(&svc);

    // Should get bond (100) + accrued rewards (100)
    assert_eq!(balance_after - balance_before, 200_0000000);
}

// ---------------------------------------------------------------------------
// Reward period overlap (notify during active period)
// ---------------------------------------------------------------------------

#[test]
fn test_notify_during_active_period() {
    let t = Setup::new();
    let svc = t.service();
    t.registry.register(&svc, &t.meta1(), &100_0000000);

    // First: 100 tokens over 1000s
    t.registry.notify_reward_amount(&100_0000000, &1000);
    t.advance(500); // half consumed: 50 distributed, 50 remaining

    // Second: 200 tokens over 1000s, leftover 50 rolls in → rate = 250/1000
    t.registry.notify_reward_amount(&200_0000000, &1000);
    t.advance(1000);

    let claimed = t.registry.claim_rewards(&svc);
    // First 500s: 50 tokens, second 1000s: 250 tokens = 300 total
    assert_eq!(claimed, 300_0000000);
}

// ---------------------------------------------------------------------------
// Slash during active reward distribution
// ---------------------------------------------------------------------------

#[test]
fn test_slash_during_rewards() {
    let t = Setup::new();
    let svc1 = t.service();
    let svc2 = t.service();
    let reporter = Address::generate(&t.env);

    t.registry.register(&svc1, &t.meta1(), &100_0000000);
    t.registry.register(&svc2, &t.meta2(), &100_0000000);

    // 100 tokens over 1000s, split 50/50
    t.registry.notify_reward_amount(&100_0000000, &1000);
    t.advance(500);

    // Slash svc1 by 50 — total bonds drops from 200 to 150
    t.registry.slash(&t.admin, &svc1, &50_0000000, &reporter);

    t.advance(500);

    let c1 = t.registry.claim_rewards(&svc1);
    let c2 = t.registry.claim_rewards(&svc2);

    // svc1: 500s at 50/200 share + 500s at 50/150 share
    // svc1: 25 + 16.666... = 41.666... ≈ 41_6666666
    // svc2: 25 + 33.333... = 58.333... ≈ 58_3333333
    // Allow for integer rounding
    assert!(c1 >= 41_0000000 && c1 <= 42_0000000);
    assert!(c2 >= 58_0000000 && c2 <= 59_0000000);
}

// ---------------------------------------------------------------------------
// Edge cases
// ---------------------------------------------------------------------------

#[test]
#[should_panic]
fn test_slash_zero_amount() {
    let t = Setup::new();
    let svc = t.service();
    let reporter = Address::generate(&t.env);
    t.registry.register(&svc, &t.meta1(), &50_0000000);
    t.registry.slash(&t.admin, &svc, &0, &reporter);
}

#[test]
#[should_panic]
fn test_register_negative_amount() {
    let t = Setup::new();
    let svc = t.service();
    t.registry.register(&svc, &t.meta1(), &-10_0000000);
}

#[test]
#[should_panic]
fn test_stake_not_registered() {
    let t = Setup::new();
    let svc = Address::generate(&t.env);
    t.registry.stake(&svc, &10_0000000);
}

#[test]
fn test_reputation_slash_recover_slash() {
    let t = Setup::new();
    let svc = t.service();
    let reporter = Address::generate(&t.env);
    t.registry.register(&svc, &t.meta1(), &200_0000000);

    // First slash: 10000 → 9000
    t.registry.slash(&t.admin, &svc, &10_0000000, &reporter);
    let info = t.registry.get(&svc);
    assert_eq!(info.reputation, 9000);

    // Recover for 10 days: 9000 + 500 = 9500
    t.advance(10 * 86400);
    let info = t.registry.get(&svc);
    assert_eq!(info.reputation, 9500);

    // Second slash: 9500 → 8500
    t.registry.slash(&t.admin, &svc, &10_0000000, &reporter);
    let info = t.registry.get(&svc);
    assert_eq!(info.reputation, 8500);
}

#[test]
fn test_withdraw_after_partial_slash_during_unbonding() {
    let t = Setup::new();
    let svc = t.service();
    let reporter = Address::generate(&t.env);
    t.registry.register(&svc, &t.meta1(), &100_0000000);

    t.registry.deregister(&svc);
    // Slash 40 during cooldown — pending_unstake capped to 60
    t.registry.slash(&t.admin, &svc, &40_0000000, &reporter);

    let info = t.registry.get(&svc);
    assert_eq!(info.bond, 60_0000000);
    assert_eq!(info.pending_unstake, 60_0000000);

    t.advance(604_800);
    t.registry.withdraw(&svc);

    // Bond fully withdrawn
    assert_eq!(t.registry.total_bonds(), 0);
}
