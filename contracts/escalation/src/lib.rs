#![no_std]

mod errors;
mod types;

#[cfg(test)]
mod test;

use errors::ContractError;
use types::{DataKey, DisputeInfo, DisputeStatus, Verdict};

use soroban_sdk::{
    contract, contractevent, contractimpl, token, vec, Address, Bytes, BytesN, Env, IntoVal,
    Symbol, Val, Vec,
};

// TTL constants (~30 days at 5s ledgers)
const TTL_THRESHOLD: u32 = 17280; // ~1 day
const TTL_EXTEND: u32 = 518400; // ~30 days

// Progressive slashing basis points
const BASE_SLASH_BPS: u32 = 1000; // 10% base
const PROGRESSIVE_SLASH_BPS: u32 = 500; // +5% per additional offense
const MAX_SLASH_BPS: u32 = 10000; // 100% max
const BPS: i128 = 10000;

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DisputeEvent {
    pub id: u64,
    pub client: Address,
    pub service: Address,
    pub bond: i128,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerdictEvent {
    pub id: u64,
    pub verdict: Verdict,
    pub slash_amount: i128,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DefaultResolveEvent {
    pub id: u64,
    pub slash_amount: i128,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SetEvaluatorEvent {
    pub pub_key: BytesN<32>,
}

// ---------------------------------------------------------------------------
// Storage helpers
// ---------------------------------------------------------------------------

fn get_admin(env: &Env) -> Address {
    env.storage().instance().get(&DataKey::Admin).unwrap()
}

fn get_registry(env: &Env) -> Address {
    env.storage().instance().get(&DataKey::Registry).unwrap()
}

fn get_token(env: &Env) -> Address {
    env.storage().instance().get(&DataKey::Token).unwrap()
}

fn get_evaluator_pub_key(env: &Env) -> BytesN<32> {
    env.storage()
        .instance()
        .get(&DataKey::EvaluatorPubKey)
        .unwrap()
}

fn get_dispute_bond(env: &Env) -> i128 {
    env.storage()
        .instance()
        .get(&DataKey::DisputeBond)
        .unwrap()
}

fn get_defense_window(env: &Env) -> u64 {
    env.storage()
        .instance()
        .get(&DataKey::DefenseWindow)
        .unwrap()
}

fn get_next_id(env: &Env) -> u64 {
    env.storage()
        .instance()
        .get(&DataKey::NextDisputeId)
        .unwrap_or(1)
}

fn load_dispute(env: &Env, id: u64) -> Option<DisputeInfo> {
    env.storage().persistent().get(&DataKey::Dispute(id))
}

fn save_dispute(env: &Env, id: u64, info: &DisputeInfo) {
    let key = DataKey::Dispute(id);
    env.storage().persistent().set(&key, info);
    env.storage()
        .persistent()
        .extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND);
}

fn get_offense_count(env: &Env, service: &Address) -> u32 {
    env.storage()
        .persistent()
        .get(&DataKey::OffenseCount(service.clone()))
        .unwrap_or(0)
}

fn set_offense_count(env: &Env, service: &Address, count: u32) {
    let key = DataKey::OffenseCount(service.clone());
    env.storage().persistent().set(&key, &count);
    env.storage()
        .persistent()
        .extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND);
}

fn bump_instance(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(TTL_THRESHOLD, TTL_EXTEND);
}

/// Compute progressive slash basis points from offense history.
/// 1st offense = 10%, 2nd = 15%, 3rd = 20%, ... up to 100%.
fn compute_slash_bps(offense_count: u32) -> u32 {
    (BASE_SLASH_BPS + offense_count.saturating_mul(PROGRESSIVE_SLASH_BPS)).min(MAX_SLASH_BPS)
}

// ---------------------------------------------------------------------------
// Cross-contract helpers
// ---------------------------------------------------------------------------

/// Get service bond amount from Registry.
fn get_service_bond(env: &Env, registry: &Address, service: &Address) -> i128 {
    let args: Vec<Val> = vec![env, service.into_val(env)];
    env.invoke_contract::<i128>(registry, &Symbol::new(env, "bond_of"), args)
}

/// Call Registry.slash. Escalation contract authorizes itself as caller.
fn call_registry_slash(
    env: &Env,
    registry: &Address,
    service: &Address,
    amount: i128,
    reporter: &Address,
) {
    let self_addr = env.current_contract_address();
    let args: Vec<Val> = vec![
        env,
        self_addr.into_val(env),
        service.into_val(env),
        amount.into_val(env),
        reporter.into_val(env),
    ];
    let _: Val = env.invoke_contract(registry, &Symbol::new(env, "slash"), args);
}

/// Execute client-wins resolution: progressive slash + return dispute bond.
fn resolve_client_wins(env: &Env, dispute: &DisputeInfo) -> i128 {
    let registry = get_registry(env);
    let offense_count = get_offense_count(env, &dispute.service);
    let slash_bps = compute_slash_bps(offense_count) as i128;

    let service_bond = get_service_bond(env, &registry, &dispute.service);
    let slash_amount = service_bond * slash_bps / BPS;

    if slash_amount > 0 {
        call_registry_slash(env, &registry, &dispute.service, slash_amount, &dispute.client);
    }

    // Track offense
    set_offense_count(env, &dispute.service, offense_count + 1);

    // Return dispute bond to client
    let tok = token::Client::new(env, &get_token(env));
    tok.transfer(
        &env.current_contract_address(),
        &dispute.client,
        &dispute.dispute_bond,
    );

    slash_amount
}

/// Execute service-wins resolution: client forfeits dispute bond to service.
fn resolve_service_wins(env: &Env, dispute: &DisputeInfo) {
    let tok = token::Client::new(env, &get_token(env));
    tok.transfer(
        &env.current_contract_address(),
        &dispute.service,
        &dispute.dispute_bond,
    );
}

// ---------------------------------------------------------------------------
// Contract
// ---------------------------------------------------------------------------

#[contract]
pub struct Escalation;

#[contractimpl]
impl Escalation {
    /// Runs once at deployment.
    pub fn __constructor(
        env: Env,
        admin: Address,
        registry: Address,
        token: Address,
        evaluator_pub_key: BytesN<32>,
        dispute_bond: i128,
        defense_window: u64,
    ) {
        assert!(dispute_bond > 0, "dispute_bond must be positive");
        assert!(defense_window > 0, "defense_window must be positive");

        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Registry, &registry);
        env.storage().instance().set(&DataKey::Token, &token);
        env.storage()
            .instance()
            .set(&DataKey::EvaluatorPubKey, &evaluator_pub_key);
        env.storage()
            .instance()
            .set(&DataKey::DisputeBond, &dispute_bond);
        env.storage()
            .instance()
            .set(&DataKey::DefenseWindow, &defense_window);
        env.storage()
            .instance()
            .set(&DataKey::NextDisputeId, &1u64);
    }

    // -----------------------------------------------------------------------
    // Dispute lifecycle
    // -----------------------------------------------------------------------

    /// File a dispute. Client deposits `dispute_bond` tokens.
    /// Returns the dispute ID.
    pub fn dispute(
        env: Env,
        client: Address,
        service: Address,
        receipt_hash: BytesN<32>,
        encrypted_response: Bytes,
    ) -> Result<u64, ContractError> {
        client.require_auth();

        let bond = get_dispute_bond(&env);

        // Transfer dispute bond from client to contract
        let tok = token::Client::new(&env, &get_token(&env));
        tok.transfer(&client, &env.current_contract_address(), &bond);

        let id = get_next_id(&env);

        let info = DisputeInfo {
            id,
            client: client.clone(),
            service: service.clone(),
            receipt_hash,
            encrypted_response,
            dispute_bond: bond,
            filed_at: env.ledger().timestamp(),
            status: DisputeStatus::Open,
        };

        save_dispute(&env, id, &info);
        env.storage()
            .instance()
            .set(&DataKey::NextDisputeId, &(id + 1));

        bump_instance(&env);
        DisputeEvent {
            id,
            client,
            service,
            bond,
        }
        .publish(&env);
        Ok(id)
    }

    /// TEE evaluator submits Ed25519-signed verdict, resolving the dispute.
    /// Message: dispute_id (8 bytes BE) ++ verdict (1 byte: 0=ClientWins, 1=ServiceWins).
    pub fn submit_verdict(
        env: Env,
        dispute_id: u64,
        verdict: Verdict,
        signature: BytesN<64>,
    ) -> Result<(), ContractError> {
        let mut dispute =
            load_dispute(&env, dispute_id).ok_or(ContractError::DisputeNotFound)?;

        if dispute.status != DisputeStatus::Open {
            return Err(ContractError::DisputeAlreadyResolved);
        }

        // Verify Ed25519 signature from TEE evaluator
        let pub_key = get_evaluator_pub_key(&env);
        let verdict_byte: u8 = match verdict {
            Verdict::ClientWins => 0,
            Verdict::ServiceWins => 1,
        };

        let mut msg_bytes = [0u8; 9];
        msg_bytes[..8].copy_from_slice(&dispute_id.to_be_bytes());
        msg_bytes[8] = verdict_byte;
        let message = Bytes::from_slice(&env, &msg_bytes);

        // Panics on invalid signature
        env.crypto()
            .ed25519_verify(&pub_key, &message, &signature);

        // Execute resolution
        let slash_amount = match verdict {
            Verdict::ClientWins => resolve_client_wins(&env, &dispute),
            Verdict::ServiceWins => {
                resolve_service_wins(&env, &dispute);
                0
            }
        };

        dispute.status = DisputeStatus::Resolved;
        save_dispute(&env, dispute_id, &dispute);

        bump_instance(&env);
        VerdictEvent {
            id: dispute_id,
            verdict,
            slash_amount,
        }
        .publish(&env);
        Ok(())
    }

    /// Auto-resolve after defense window expires with no verdict.
    /// Server loses by default. Anyone can call this.
    pub fn resolve(env: Env, dispute_id: u64) -> Result<(), ContractError> {
        let mut dispute =
            load_dispute(&env, dispute_id).ok_or(ContractError::DisputeNotFound)?;

        if dispute.status != DisputeStatus::Open {
            return Err(ContractError::DisputeAlreadyResolved);
        }

        let defense_window = get_defense_window(&env);
        let now = env.ledger().timestamp();

        if now < dispute.filed_at.checked_add(defense_window).unwrap() {
            return Err(ContractError::DefenseWindowActive);
        }

        // Server didn't defend — default loss
        let slash_amount = resolve_client_wins(&env, &dispute);

        dispute.status = DisputeStatus::Resolved;
        save_dispute(&env, dispute_id, &dispute);

        bump_instance(&env);
        DefaultResolveEvent {
            id: dispute_id,
            slash_amount,
        }
        .publish(&env);
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Admin
    // -----------------------------------------------------------------------

    /// Update the TEE evaluator's Ed25519 public key.
    pub fn set_evaluator_pub_key(env: Env, pub_key: BytesN<32>) {
        get_admin(&env).require_auth();
        env.storage()
            .instance()
            .set(&DataKey::EvaluatorPubKey, &pub_key);
        bump_instance(&env);
        SetEvaluatorEvent { pub_key }.publish(&env);
    }

    /// Update minimum dispute bond.
    pub fn set_dispute_bond(env: Env, bond: i128) {
        get_admin(&env).require_auth();
        assert!(bond > 0, "dispute_bond must be positive");
        env.storage().instance().set(&DataKey::DisputeBond, &bond);
        bump_instance(&env);
    }

    // -----------------------------------------------------------------------
    // Read-only
    // -----------------------------------------------------------------------

    pub fn get_dispute(env: Env, dispute_id: u64) -> Result<DisputeInfo, ContractError> {
        bump_instance(&env);
        load_dispute(&env, dispute_id).ok_or(ContractError::DisputeNotFound)
    }

    pub fn offense_count(env: Env, service: Address) -> u32 {
        bump_instance(&env);
        get_offense_count(&env, &service)
    }

    pub fn dispute_bond_amount(env: Env) -> i128 {
        bump_instance(&env);
        get_dispute_bond(&env)
    }
}
