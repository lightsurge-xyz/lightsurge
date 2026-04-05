#![cfg(test)]
extern crate std;

use crate::{
    types::{DisputeStatus, Verdict},
    Escalation, EscalationClient,
};
use ed25519_dalek::{Signer, SigningKey};
use lightsurge_registry::{Registry, RegistryClient};
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token, Address, Bytes, BytesN, Env,
};

// ---------------------------------------------------------------------------
// Ed25519 helpers
// ---------------------------------------------------------------------------

fn make_keypair(env: &Env) -> (BytesN<32>, SigningKey) {
    let secret = [42u8; 32];
    let signing_key = SigningKey::from_bytes(&secret);
    let pub_key = signing_key.verifying_key().to_bytes();
    (BytesN::from_array(env, &pub_key), signing_key)
}

fn sign_verdict(env: &Env, key: &SigningKey, dispute_id: u64, verdict: &Verdict) -> BytesN<64> {
    let verdict_byte: u8 = match verdict {
        Verdict::ClientWins => 0,
        Verdict::ServiceWins => 1,
    };
    let mut msg = [0u8; 9];
    msg[..8].copy_from_slice(&dispute_id.to_be_bytes());
    msg[8] = verdict_byte;
    let sig = key.sign(&msg);
    BytesN::from_array(env, &sig.to_bytes())
}

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
    #[allow(dead_code)]
    admin: Address,
    token: token::Client<'a>,
    token_sac: token::StellarAssetClient<'a>,
    registry: RegistryClient<'a>,
    escalation: EscalationClient<'a>,
    signing_key: SigningKey,
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

        // Deploy Registry
        let min_bond: i128 = 10_0000000; // 10 tokens
        let cooldown: u64 = 604_800; // 7 days
        let registry_id =
            env.register(Registry, (&admin, &token.address, &min_bond, &cooldown));
        let registry = RegistryClient::new(&env, &registry_id);

        // Deploy Escalation
        let (pub_key, signing_key) = make_keypair(&env);
        let dispute_bond: i128 = 5_0000000; // 5 tokens
        let defense_window: u64 = 86400; // 24 hours
        let escalation_id = env.register(
            Escalation,
            (
                &admin,
                &registry_id,
                &token.address,
                &pub_key,
                &dispute_bond,
                &defense_window,
            ),
        );
        let escalation = EscalationClient::new(&env, &escalation_id);

        // Wire: set escalation as dispute resolver on registry
        registry.set_dispute_resolver(&escalation_id);

        // Set treasury so slash distribution is testable
        let treasury = Address::generate(&env);
        registry.set_treasury(&treasury);

        // Fund admin for reward seeding
        token_sac.mint(&admin, &10_000_0000000);

        Setup {
            env,
            admin,
            token,
            token_sac,
            registry,
            escalation,
            signing_key,
        }
    }

    /// Create a funded service, register it on the registry.
    fn registered_service(&self, bond: i128) -> Address {
        let addr = Address::generate(&self.env);
        self.token_sac.mint(&addr, &1_000_0000000);
        let meta = Bytes::from_slice(&self.env, b"service-meta");
        self.registry.register(&addr, &meta, &bond);
        addr
    }

    /// Create a funded client.
    fn client(&self) -> Address {
        let addr = Address::generate(&self.env);
        self.token_sac.mint(&addr, &1_000_0000000);
        addr
    }

    fn advance(&self, seconds: u64) {
        self.env.ledger().with_mut(|li| {
            li.timestamp += seconds;
        });
    }

    fn receipt_hash(&self) -> BytesN<32> {
        BytesN::from_array(&self.env, &[0xABu8; 32])
    }

    fn encrypted_response(&self) -> Bytes {
        Bytes::from_slice(&self.env, b"encrypted-response-data")
    }
}

// ---------------------------------------------------------------------------
// Dispute filing
// ---------------------------------------------------------------------------

#[test]
fn test_dispute_filing() {
    let t = Setup::new();
    let svc = t.registered_service(100_0000000);
    let client = t.client();

    let balance_before = t.token.balance(&client);
    let id = t
        .escalation
        .dispute(&client, &svc, &t.receipt_hash(), &t.encrypted_response());
    assert_eq!(id, 1);

    // Client charged dispute bond (5 tokens)
    assert_eq!(t.token.balance(&client), balance_before - 5_0000000);

    // Dispute stored
    let info = t.escalation.get_dispute(&1);
    assert_eq!(info.client, client);
    assert_eq!(info.service, svc);
    assert_eq!(info.dispute_bond, 5_0000000);
    assert_eq!(info.status, DisputeStatus::Open);
    assert_eq!(info.filed_at, 1_000_000);
}

#[test]
fn test_dispute_ids_increment() {
    let t = Setup::new();
    let svc = t.registered_service(100_0000000);
    let client = t.client();

    let id1 = t
        .escalation
        .dispute(&client, &svc, &t.receipt_hash(), &t.encrypted_response());
    let id2 = t
        .escalation
        .dispute(&client, &svc, &t.receipt_hash(), &t.encrypted_response());
    assert_eq!(id1, 1);
    assert_eq!(id2, 2);
}

// ---------------------------------------------------------------------------
// Verdict: client wins
// ---------------------------------------------------------------------------

#[test]
fn test_verdict_client_wins() {
    let t = Setup::new();
    let svc = t.registered_service(100_0000000);
    let client = t.client();

    let id = t
        .escalation
        .dispute(&client, &svc, &t.receipt_hash(), &t.encrypted_response());

    let verdict = Verdict::ClientWins;
    let sig = sign_verdict(&t.env, &t.signing_key, id, &verdict);

    let client_before = t.token.balance(&client);
    t.escalation.submit_verdict(&id, &verdict, &sig);

    // Client gets dispute bond back (5 tokens)
    // Client also gets 30% of slash as reporter reward from registry
    // Slash = 10% of 100 = 10 tokens. Reporter = 30% of 10 = 3 tokens
    let client_after = t.token.balance(&client);
    assert_eq!(client_after - client_before, 5_0000000 + 3_0000000); // bond + reporter reward

    // Service bond reduced: 100 - 10 = 90
    let svc_info = t.registry.get(&svc);
    assert_eq!(svc_info.bond, 90_0000000);

    // Dispute resolved
    let dispute = t.escalation.get_dispute(&id);
    assert_eq!(dispute.status, DisputeStatus::Resolved);

    // Offense count = 1
    assert_eq!(t.escalation.offense_count(&svc), 1);
}

// ---------------------------------------------------------------------------
// Verdict: service wins
// ---------------------------------------------------------------------------

#[test]
fn test_verdict_service_wins() {
    let t = Setup::new();
    let svc = t.registered_service(100_0000000);
    let client = t.client();

    let id = t
        .escalation
        .dispute(&client, &svc, &t.receipt_hash(), &t.encrypted_response());

    let verdict = Verdict::ServiceWins;
    let sig = sign_verdict(&t.env, &t.signing_key, id, &verdict);

    let svc_before = t.token.balance(&svc);
    t.escalation.submit_verdict(&id, &verdict, &sig);

    // Service gets client's dispute bond (5 tokens)
    let svc_after = t.token.balance(&svc);
    assert_eq!(svc_after - svc_before, 5_0000000);

    // Service bond unchanged
    let svc_info = t.registry.get(&svc);
    assert_eq!(svc_info.bond, 100_0000000);

    // No offense recorded
    assert_eq!(t.escalation.offense_count(&svc), 0);
}

// ---------------------------------------------------------------------------
// Auto-resolve on timeout
// ---------------------------------------------------------------------------

#[test]
fn test_auto_resolve_timeout() {
    let t = Setup::new();
    let svc = t.registered_service(100_0000000);
    let client = t.client();

    let id = t
        .escalation
        .dispute(&client, &svc, &t.receipt_hash(), &t.encrypted_response());

    // Advance past defense window (24h)
    t.advance(86400);

    let client_before = t.token.balance(&client);
    t.escalation.resolve(&id);

    // Client gets bond back + reporter reward (same as client_wins)
    let client_after = t.token.balance(&client);
    assert_eq!(client_after - client_before, 5_0000000 + 3_0000000);

    // Service slashed
    let svc_info = t.registry.get(&svc);
    assert_eq!(svc_info.bond, 90_0000000);

    assert_eq!(t.escalation.offense_count(&svc), 1);
}

#[test]
#[should_panic]
fn test_resolve_before_window_fails() {
    let t = Setup::new();
    let svc = t.registered_service(100_0000000);
    let client = t.client();

    let id = t
        .escalation
        .dispute(&client, &svc, &t.receipt_hash(), &t.encrypted_response());

    // Only 1 hour, need 24h
    t.advance(3600);
    t.escalation.resolve(&id);
}

// ---------------------------------------------------------------------------
// Progressive slashing
// ---------------------------------------------------------------------------

#[test]
fn test_progressive_slashing() {
    let t = Setup::new();
    let svc = t.registered_service(200_0000000);
    let client = t.client();

    // 1st dispute: 10% slash = 20 tokens
    let id1 = t
        .escalation
        .dispute(&client, &svc, &t.receipt_hash(), &t.encrypted_response());
    let verdict = Verdict::ClientWins;
    let sig1 = sign_verdict(&t.env, &t.signing_key, id1, &verdict);
    t.escalation.submit_verdict(&id1, &verdict, &sig1);

    let info = t.registry.get(&svc);
    assert_eq!(info.bond, 180_0000000); // 200 - 20
    assert_eq!(t.escalation.offense_count(&svc), 1);

    // 2nd dispute: 15% of 180 = 27 tokens
    let id2 = t
        .escalation
        .dispute(&client, &svc, &t.receipt_hash(), &t.encrypted_response());
    let sig2 = sign_verdict(&t.env, &t.signing_key, id2, &verdict);
    t.escalation.submit_verdict(&id2, &verdict, &sig2);

    let info = t.registry.get(&svc);
    assert_eq!(info.bond, 153_0000000); // 180 - 27
    assert_eq!(t.escalation.offense_count(&svc), 2);

    // 3rd dispute: 20% of 153 = 30.6 → 30_6000000
    let id3 = t
        .escalation
        .dispute(&client, &svc, &t.receipt_hash(), &t.encrypted_response());
    let sig3 = sign_verdict(&t.env, &t.signing_key, id3, &verdict);
    t.escalation.submit_verdict(&id3, &verdict, &sig3);

    let info = t.registry.get(&svc);
    assert_eq!(info.bond, 122_4000000); // 153 - 30.6
    assert_eq!(t.escalation.offense_count(&svc), 3);
}

// ---------------------------------------------------------------------------
// Ed25519 signature validation
// ---------------------------------------------------------------------------

#[test]
#[should_panic]
fn test_invalid_signature_fails() {
    let t = Setup::new();
    let svc = t.registered_service(100_0000000);
    let client = t.client();

    let id = t
        .escalation
        .dispute(&client, &svc, &t.receipt_hash(), &t.encrypted_response());

    // Sign with a wrong key
    let wrong_key = SigningKey::from_bytes(&[99u8; 32]);
    let verdict = Verdict::ClientWins;
    let bad_sig = sign_verdict(&t.env, &wrong_key, id, &verdict);

    t.escalation.submit_verdict(&id, &verdict, &bad_sig);
}

#[test]
#[should_panic]
fn test_wrong_verdict_in_signature_fails() {
    let t = Setup::new();
    let svc = t.registered_service(100_0000000);
    let client = t.client();

    let id = t
        .escalation
        .dispute(&client, &svc, &t.receipt_hash(), &t.encrypted_response());

    // Sign for ServiceWins but submit as ClientWins
    let sig = sign_verdict(&t.env, &t.signing_key, id, &Verdict::ServiceWins);
    t.escalation
        .submit_verdict(&id, &Verdict::ClientWins, &sig);
}

// ---------------------------------------------------------------------------
// Edge cases
// ---------------------------------------------------------------------------

#[test]
#[should_panic]
fn test_double_resolve_fails() {
    let t = Setup::new();
    let svc = t.registered_service(100_0000000);
    let client = t.client();

    let id = t
        .escalation
        .dispute(&client, &svc, &t.receipt_hash(), &t.encrypted_response());

    let verdict = Verdict::ClientWins;
    let sig = sign_verdict(&t.env, &t.signing_key, id, &verdict);
    t.escalation.submit_verdict(&id, &verdict, &sig);

    // Second resolve fails
    t.advance(86400);
    t.escalation.resolve(&id);
}

#[test]
#[should_panic]
fn test_double_verdict_fails() {
    let t = Setup::new();
    let svc = t.registered_service(100_0000000);
    let client = t.client();

    let id = t
        .escalation
        .dispute(&client, &svc, &t.receipt_hash(), &t.encrypted_response());

    let verdict = Verdict::ClientWins;
    let sig = sign_verdict(&t.env, &t.signing_key, id, &verdict);
    t.escalation.submit_verdict(&id, &verdict, &sig);
    t.escalation.submit_verdict(&id, &verdict, &sig); // fails
}

#[test]
#[should_panic]
fn test_dispute_not_found() {
    let t = Setup::new();
    t.escalation.get_dispute(&999);
}

#[test]
fn test_verdict_can_happen_before_window() {
    let t = Setup::new();
    let svc = t.registered_service(100_0000000);
    let client = t.client();

    let id = t
        .escalation
        .dispute(&client, &svc, &t.receipt_hash(), &t.encrypted_response());

    // Verdict at 1 second — well within window
    t.advance(1);
    let verdict = Verdict::ClientWins;
    let sig = sign_verdict(&t.env, &t.signing_key, id, &verdict);
    t.escalation.submit_verdict(&id, &verdict, &sig);

    let dispute = t.escalation.get_dispute(&id);
    assert_eq!(dispute.status, DisputeStatus::Resolved);
}

// ---------------------------------------------------------------------------
// Integration: rewards + slash interaction
// ---------------------------------------------------------------------------

#[test]
fn test_slash_during_rewards() {
    let t = Setup::new();
    let svc = t.registered_service(100_0000000);
    let client = t.client();

    // Seed rewards: 100 tokens over 1000s
    t.registry.notify_reward_amount(&100_0000000, &1000);
    t.advance(500); // accrue 50 tokens

    // Dispute + slash
    let id = t
        .escalation
        .dispute(&client, &svc, &t.receipt_hash(), &t.encrypted_response());
    let verdict = Verdict::ClientWins;
    let sig = sign_verdict(&t.env, &t.signing_key, id, &verdict);
    t.escalation.submit_verdict(&id, &verdict, &sig);

    // Service bond slashed by 10%: 100 → 90
    let info = t.registry.get(&svc);
    assert_eq!(info.bond, 90_0000000);

    // Accrued rewards still claimable
    t.advance(500);
    let claimed = t.registry.claim_rewards(&svc);
    assert!(claimed > 0);
}

// ---------------------------------------------------------------------------
// Reputation impact
// ---------------------------------------------------------------------------

#[test]
fn test_slash_decreases_reputation() {
    let t = Setup::new();
    let svc = t.registered_service(100_0000000);
    let client = t.client();

    let id = t
        .escalation
        .dispute(&client, &svc, &t.receipt_hash(), &t.encrypted_response());
    let verdict = Verdict::ClientWins;
    let sig = sign_verdict(&t.env, &t.signing_key, id, &verdict);
    t.escalation.submit_verdict(&id, &verdict, &sig);

    // Reputation decreased by 1000 bps (10%)
    let info = t.registry.get(&svc);
    assert_eq!(info.reputation, 9000);
}
