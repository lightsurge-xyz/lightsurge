#![no_std]

mod errors;
mod types;

#[cfg(test)]
mod test;

use errors::ContractError;
use types::{DataKey, ServiceInfo, ServiceStatus};

use soroban_sdk::{contract, contractevent, contractimpl, token, Address, Bytes, Env};

// 10^18 scaling factor for reward-per-token precision
const PRECISION: i128 = 1_000_000_000_000_000_000;

// Default reputation: 100.00% (basis points)
const DEFAULT_REPUTATION: u32 = 10000;

// Reputation penalty per slash (basis points)
const SLASH_REPUTATION_PENALTY: u32 = 1000; // 10% per slash

// Reputation recovery rate (basis points per day, ~17280 ledgers at 5s)
const REPUTATION_RECOVERY_RATE: u32 = 50; // 0.50% per day (~20 days full recovery from one slash)
const SECONDS_PER_DAY: u64 = 86400;

// Slash distribution (basis points, total = 10000)
const SLASH_REPORTER_BPS: i128 = 3000; // 30% to reporter
const SLASH_TREASURY_BPS: i128 = 2000; // 20% to treasury
const BPS: i128 = 10000;

// TTL constants (~30 days at 5s ledgers)
const TTL_THRESHOLD: u32 = 17280; // ~1 day
const TTL_EXTEND: u32 = 518400; // ~30 days

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegisterEvent {
    pub service: Address,
    pub bond: i128,
    pub metadata: Bytes,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdateMetadataEvent {
    pub service: Address,
    pub metadata: Bytes,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeregisterEvent {
    pub service: Address,
    pub bond: i128,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StakeEvent {
    pub service: Address,
    pub amount: i128,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnstakeEvent {
    pub service: Address,
    pub amount: i128,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WithdrawEvent {
    pub service: Address,
    pub amount: i128,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SlashEvent {
    pub service: Address,
    pub amount: i128,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClaimEvent {
    pub service: Address,
    pub amount: i128,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RewardEvent {
    pub amount: i128,
    pub duration: u64,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SetAdminEvent {
    pub old_admin: Address,
    pub new_admin: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SetDisputeResolverEvent {
    pub resolver: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SetTreasuryEvent {
    pub treasury: Address,
}

// ---------------------------------------------------------------------------
// Storage helpers
// ---------------------------------------------------------------------------

fn get_admin(env: &Env) -> Address {
    env.storage().instance().get(&DataKey::Admin).unwrap()
}

fn get_token(env: &Env) -> Address {
    env.storage().instance().get(&DataKey::Token).unwrap()
}

fn get_min_bond(env: &Env) -> i128 {
    env.storage().instance().get(&DataKey::MinBond).unwrap()
}

fn get_cooldown(env: &Env) -> u64 {
    env.storage()
        .instance()
        .get(&DataKey::CooldownPeriod)
        .unwrap()
}

fn get_total_bonds(env: &Env) -> i128 {
    env.storage()
        .instance()
        .get(&DataKey::TotalBonds)
        .unwrap_or(0)
}

fn get_reward_rate(env: &Env) -> i128 {
    env.storage()
        .instance()
        .get(&DataKey::RewardRate)
        .unwrap_or(0)
}

fn get_rpt_stored(env: &Env) -> i128 {
    env.storage()
        .instance()
        .get(&DataKey::RewardPerTokenStored)
        .unwrap_or(0)
}

fn get_last_update(env: &Env) -> u64 {
    env.storage()
        .instance()
        .get(&DataKey::LastUpdateTime)
        .unwrap_or(0)
}

fn get_reward_end(env: &Env) -> u64 {
    env.storage()
        .instance()
        .get(&DataKey::RewardEndTime)
        .unwrap_or(0)
}

fn get_service(env: &Env, service: &Address) -> Option<ServiceInfo> {
    let key = DataKey::Service(service.clone());
    env.storage().persistent().get(&key)
}

fn set_service(env: &Env, service: &Address, info: &ServiceInfo) {
    let key = DataKey::Service(service.clone());
    env.storage().persistent().set(&key, info);
    env.storage()
        .persistent()
        .extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND);
}

fn bump_instance(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(TTL_THRESHOLD, TTL_EXTEND);
}

/// Compute effective reputation with lazy time-based recovery.
/// Reputation recovers toward 10000 (100%) at REPUTATION_RECOVERY_RATE bps/day.
fn effective_reputation(info: &ServiceInfo, now: u64) -> u32 {
    if info.reputation >= DEFAULT_REPUTATION || info.last_slash_time == 0 {
        return info.reputation.min(DEFAULT_REPUTATION);
    }
    let elapsed_days = now.saturating_sub(info.last_slash_time) / SECONDS_PER_DAY;
    let elapsed_days_u32 = u32::try_from(elapsed_days).unwrap_or(u32::MAX);
    let recovery = elapsed_days_u32.saturating_mul(REPUTATION_RECOVERY_RATE);
    (info.reputation + recovery).min(DEFAULT_REPUTATION)
}

// ---------------------------------------------------------------------------
// Synthetix reward-per-token helpers
// ---------------------------------------------------------------------------

fn reward_per_token(env: &Env) -> i128 {
    let total = get_total_bonds(env);
    let stored = get_rpt_stored(env);
    if total == 0 {
        return stored;
    }
    let end = get_reward_end(env);
    let last_update = get_last_update(env);
    let now = env.ledger().timestamp();
    let last_applicable = if now < end { now } else { end };
    if last_applicable <= last_update {
        return stored;
    }
    let elapsed = (last_applicable - last_update) as i128;
    let rate = get_reward_rate(env);
    // Compute (rate * PRECISION / total) first to avoid i128 overflow
    // on the three-way multiplication elapsed * rate * PRECISION.
    let rate_per_token = rate.checked_mul(PRECISION).unwrap() / total;
    stored
        .checked_add(elapsed.checked_mul(rate_per_token).unwrap())
        .unwrap()
}

fn earned(info: &ServiceInfo, rpt: i128) -> i128 {
    info.bond
        .checked_mul(rpt.checked_sub(info.reward_per_token_paid).unwrap())
        .unwrap()
        / PRECISION
        + info.accrued_rewards
}

/// Update global reward state and (optionally) a single service's accrued rewards.
/// Returns the updated `ServiceInfo` if a service address was provided and found.
fn update_rewards(env: &Env, service: Option<&Address>) -> Option<ServiceInfo> {
    let rpt = reward_per_token(env);
    env.storage()
        .instance()
        .set(&DataKey::RewardPerTokenStored, &rpt);

    let end = get_reward_end(env);
    if end > 0 && get_total_bonds(env) > 0 {
        let now = env.ledger().timestamp();
        let last = if now < end { now } else { end };
        env.storage()
            .instance()
            .set(&DataKey::LastUpdateTime, &last);
    }

    if let Some(addr) = service {
        if let Some(mut info) = get_service(env, addr) {
            info.accrued_rewards = earned(&info, rpt);
            info.reward_per_token_paid = rpt;
            set_service(env, addr, &info);
            return Some(info);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Contract
// ---------------------------------------------------------------------------

#[contract]
pub struct Registry;

#[contractimpl]
impl Registry {
    /// Runs once at deployment. Sets admin, bond token, minimum bond, and cooldown.
    pub fn __constructor(
        env: Env,
        admin: Address,
        token: Address,
        min_bond: i128,
        cooldown_period: u64,
    ) {
        assert!(min_bond > 0, "min_bond must be positive");
        assert!(cooldown_period > 0, "cooldown_period must be positive");

        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Token, &token);
        env.storage().instance().set(&DataKey::MinBond, &min_bond);
        env.storage()
            .instance()
            .set(&DataKey::CooldownPeriod, &cooldown_period);
        env.storage().instance().set(&DataKey::TotalBonds, &0i128);
        env.storage()
            .instance()
            .set(&DataKey::RewardPerTokenStored, &0i128);
        env.storage().instance().set(&DataKey::RewardRate, &0i128);
        env.storage()
            .instance()
            .set(&DataKey::LastUpdateTime, &0u64);
        env.storage().instance().set(&DataKey::RewardEndTime, &0u64);
    }

    // -----------------------------------------------------------------------
    // Service lifecycle
    // -----------------------------------------------------------------------

    /// Register a service. Transfers `bond_amount` tokens from caller to contract.
    /// `metadata` is the raw service descriptor; its SHA-256 hash is stored on-chain
    /// and the raw bytes are emitted in the event for off-chain indexers.
    pub fn register(
        env: Env,
        service: Address,
        metadata: Bytes,
        bond_amount: i128,
    ) -> Result<(), ContractError> {
        service.require_auth();

        if bond_amount <= 0 {
            return Err(ContractError::InvalidAmount);
        }
        if bond_amount < get_min_bond(&env) {
            return Err(ContractError::InsufficientBond);
        }
        if get_service(&env, &service).is_some() {
            return Err(ContractError::AlreadyRegistered);
        }

        update_rewards(&env, None);

        let metadata_hash = env.crypto().sha256(&metadata).into();

        // Transfer bond
        let tok = token::Client::new(&env, &get_token(&env));
        tok.transfer(&service, &env.current_contract_address(), &bond_amount);

        // Store service
        let rpt = get_rpt_stored(&env);
        set_service(
            &env,
            &service,
            &ServiceInfo {
                owner: service.clone(),
                metadata_hash,
                bond: bond_amount,
                pending_unstake: 0,
                unstake_time: 0,
                reputation: DEFAULT_REPUTATION,
                status: ServiceStatus::Active,
                registered_at: env.ledger().timestamp(),
                last_slash_time: 0,
                reward_per_token_paid: rpt,
                accrued_rewards: 0,
            },
        );

        // Update totals
        let total = get_total_bonds(&env);
        env.storage()
            .instance()
            .set(&DataKey::TotalBonds, &(total + bond_amount));

        bump_instance(&env);
        RegisterEvent {
            service,
            bond: bond_amount,
            metadata,
        }
        .publish(&env);
        Ok(())
    }

    /// Initiate full unbonding. Bond remains slashable during cooldown.
    /// Note: rewards continue accruing during unbonding since the bond is still at risk.
    pub fn deregister(env: Env, service: Address) -> Result<(), ContractError> {
        service.require_auth();

        let info = get_service(&env, &service).ok_or(ContractError::NotRegistered)?;
        if info.status != ServiceStatus::Active {
            return Err(ContractError::ServiceNotActive);
        }

        let mut info = update_rewards(&env, Some(&service)).unwrap();

        info.status = ServiceStatus::Unbonding;
        info.pending_unstake = info.bond;
        info.unstake_time = env.ledger().timestamp();
        set_service(&env, &service, &info);

        bump_instance(&env);
        DeregisterEvent {
            service,
            bond: info.bond,
        }
        .publish(&env);
        Ok(())
    }

    /// Read service profile. Reputation reflects lazy time-based recovery.
    pub fn get(env: Env, service: Address) -> Result<ServiceInfo, ContractError> {
        bump_instance(&env);
        let mut info = get_service(&env, &service).ok_or(ContractError::NotRegistered)?;
        info.reputation = effective_reputation(&info, env.ledger().timestamp());
        Ok(info)
    }

    /// Update the off-chain metadata pointer.
    /// `metadata` is the raw service descriptor; its SHA-256 hash replaces the stored one
    /// and the raw bytes are emitted for off-chain indexers.
    /// Note: allowed during Unbonding so services can correct metadata before exit.
    pub fn update_metadata(
        env: Env,
        service: Address,
        metadata: Bytes,
    ) -> Result<(), ContractError> {
        service.require_auth();

        let mut info = get_service(&env, &service).ok_or(ContractError::NotRegistered)?;
        info.metadata_hash = env.crypto().sha256(&metadata).into();
        set_service(&env, &service, &info);

        bump_instance(&env);
        UpdateMetadataEvent { service, metadata }.publish(&env);
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Staking
    // -----------------------------------------------------------------------

    /// Add to an existing bond.
    pub fn stake(env: Env, service: Address, amount: i128) -> Result<(), ContractError> {
        service.require_auth();

        if amount <= 0 {
            return Err(ContractError::InvalidAmount);
        }
        let info = get_service(&env, &service).ok_or(ContractError::NotRegistered)?;
        if info.status != ServiceStatus::Active {
            return Err(ContractError::ServiceNotActive);
        }

        let mut info = update_rewards(&env, Some(&service)).unwrap();

        // Transfer tokens
        let tok = token::Client::new(&env, &get_token(&env));
        tok.transfer(&service, &env.current_contract_address(), &amount);

        info.bond = info.bond.checked_add(amount).unwrap();
        set_service(&env, &service, &info);

        let total = get_total_bonds(&env);
        env.storage()
            .instance()
            .set(&DataKey::TotalBonds, &(total + amount));

        bump_instance(&env);
        StakeEvent { service, amount }.publish(&env);
        Ok(())
    }

    /// Initiate partial unbonding. One pending unstake at a time.
    /// Remaining bond must stay >= min_bond.
    pub fn unstake(env: Env, service: Address, amount: i128) -> Result<(), ContractError> {
        service.require_auth();

        if amount <= 0 {
            return Err(ContractError::InvalidAmount);
        }
        let info = get_service(&env, &service).ok_or(ContractError::NotRegistered)?;
        if info.status != ServiceStatus::Active {
            return Err(ContractError::ServiceNotActive);
        }
        if info.pending_unstake > 0 {
            return Err(ContractError::PendingUnstakeExists);
        }
        let remaining = info
            .bond
            .checked_sub(amount)
            .ok_or(ContractError::InvalidAmount)?;
        if remaining < get_min_bond(&env) {
            return Err(ContractError::BondBelowMinimum);
        }

        let mut info = update_rewards(&env, Some(&service)).unwrap();

        info.pending_unstake = amount;
        info.unstake_time = env.ledger().timestamp();
        set_service(&env, &service, &info);

        bump_instance(&env);
        UnstakeEvent { service, amount }.publish(&env);
        Ok(())
    }

    /// Withdraw tokens after cooldown.
    pub fn withdraw(env: Env, service: Address) -> Result<(), ContractError> {
        service.require_auth();

        let info = get_service(&env, &service).ok_or(ContractError::NotRegistered)?;
        if info.pending_unstake == 0 {
            return Err(ContractError::NoPendingUnstake);
        }
        if env.ledger().timestamp() < info.unstake_time.checked_add(get_cooldown(&env)).unwrap() {
            return Err(ContractError::CooldownNotPassed);
        }

        let mut info = update_rewards(&env, Some(&service)).unwrap();

        let amount = info.pending_unstake;

        // Transfer tokens back
        let tok = token::Client::new(&env, &get_token(&env));
        tok.transfer(&env.current_contract_address(), &service, &amount);

        info.bond = info.bond.checked_sub(amount).unwrap();
        info.pending_unstake = 0;
        info.unstake_time = 0;

        // Update total bonds
        let total = get_total_bonds(&env);
        env.storage()
            .instance()
            .set(&DataKey::TotalBonds, &total.checked_sub(amount).unwrap());

        // Full deregistration: pay out any accrued rewards, then remove record
        if info.status == ServiceStatus::Unbonding && info.bond == 0 {
            if info.accrued_rewards > 0 {
                tok.transfer(
                    &env.current_contract_address(),
                    &service,
                    &info.accrued_rewards,
                );
            }
            env.storage()
                .persistent()
                .remove(&DataKey::Service(service.clone()));
        } else {
            set_service(&env, &service, &info);
        }

        bump_instance(&env);
        WithdrawEvent { service, amount }.publish(&env);
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Slashing
    // -----------------------------------------------------------------------

    /// Slash a service's bond. Only callable by admin or dispute resolver.
    /// Distribution: 30% reporter, 20% treasury, 50% stays in contract
    /// (admin can redistribute via `notify_reward_amount`).
    pub fn slash(
        env: Env,
        caller: Address,
        service: Address,
        amount: i128,
        reporter: Address,
    ) -> Result<(), ContractError> {
        caller.require_auth();

        // Authorize caller
        let admin = get_admin(&env);
        let resolver: Option<Address> = env.storage().instance().get(&DataKey::DisputeResolver);
        let authorized = caller == admin || resolver.map_or(false, |r| caller == r);
        if !authorized {
            return Err(ContractError::Unauthorized);
        }

        if amount <= 0 {
            return Err(ContractError::InvalidAmount);
        }

        let mut info = update_rewards(&env, Some(&service)).ok_or(ContractError::NotRegistered)?;

        // Cap to available bond
        let slash_amount = if amount > info.bond {
            info.bond
        } else {
            amount
        };
        if slash_amount == 0 {
            return Err(ContractError::NothingToSlash);
        }

        info.bond = info.bond.checked_sub(slash_amount).unwrap();

        // Decrease reputation: apply recovery first, then penalty
        let now = env.ledger().timestamp();
        let current_rep = effective_reputation(&info, now);
        info.reputation = current_rep.saturating_sub(SLASH_REPUTATION_PENALTY);
        info.last_slash_time = now;

        // Cap pending unstake to remaining bond
        if info.pending_unstake > info.bond {
            info.pending_unstake = info.bond;
        }

        // Update total bonds
        let total = get_total_bonds(&env);
        env.storage().instance().set(
            &DataKey::TotalBonds,
            &total.checked_sub(slash_amount).unwrap(),
        );

        // Distribute slashed funds
        let to_reporter = slash_amount * SLASH_REPORTER_BPS / BPS;
        let to_treasury = slash_amount * SLASH_TREASURY_BPS / BPS;
        // Remainder (50%) stays in contract; admin can redistribute via notify_reward_amount

        let tok = token::Client::new(&env, &get_token(&env));

        if to_reporter > 0 {
            tok.transfer(&env.current_contract_address(), &reporter, &to_reporter);
        }

        if to_treasury > 0 {
            let treasury: Option<Address> = env.storage().instance().get(&DataKey::Treasury);
            if let Some(treasury_addr) = treasury {
                tok.transfer(
                    &env.current_contract_address(),
                    &treasury_addr,
                    &to_treasury,
                );
            }
            // If no treasury set, funds stay in contract
        }

        // Bond depleted → pay out any accrued rewards, then remove
        if info.bond == 0 {
            if info.accrued_rewards > 0 {
                tok.transfer(
                    &env.current_contract_address(),
                    &info.owner,
                    &info.accrued_rewards,
                );
            }
            env.storage()
                .persistent()
                .remove(&DataKey::Service(service.clone()));
        } else {
            set_service(&env, &service, &info);
        }

        bump_instance(&env);
        SlashEvent {
            service,
            amount: slash_amount,
        }
        .publish(&env);
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Rewards
    // -----------------------------------------------------------------------

    /// Claim accrued APY rewards.
    pub fn claim_rewards(env: Env, service: Address) -> Result<i128, ContractError> {
        service.require_auth();

        let mut info = update_rewards(&env, Some(&service)).ok_or(ContractError::NotRegistered)?;

        let rewards = info.accrued_rewards;
        if rewards == 0 {
            return Ok(0);
        }

        info.accrued_rewards = 0;
        set_service(&env, &service, &info);

        let tok = token::Client::new(&env, &get_token(&env));
        tok.transfer(&env.current_contract_address(), &service, &rewards);

        bump_instance(&env);
        ClaimEvent {
            service,
            amount: rewards,
        }
        .publish(&env);
        Ok(rewards)
    }

    // -----------------------------------------------------------------------
    // Admin
    // -----------------------------------------------------------------------

    /// Transfer admin role.
    pub fn set_admin(env: Env, new_admin: Address) {
        let old_admin = get_admin(&env);
        old_admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        bump_instance(&env);
        SetAdminEvent {
            old_admin,
            new_admin,
        }
        .publish(&env);
    }

    /// Set the dispute resolver contract address.
    pub fn set_dispute_resolver(env: Env, resolver: Address) {
        get_admin(&env).require_auth();
        env.storage()
            .instance()
            .set(&DataKey::DisputeResolver, &resolver);
        bump_instance(&env);
        SetDisputeResolverEvent { resolver }.publish(&env);
    }

    /// Set the treasury address for slash distributions.
    pub fn set_treasury(env: Env, treasury: Address) {
        get_admin(&env).require_auth();
        env.storage().instance().set(&DataKey::Treasury, &treasury);
        bump_instance(&env);
        SetTreasuryEvent { treasury }.publish(&env);
    }

    /// Update the minimum bond required for registration.
    pub fn set_min_bond(env: Env, new_min_bond: i128) {
        get_admin(&env).require_auth();
        assert!(new_min_bond > 0, "min_bond must be positive");
        env.storage()
            .instance()
            .set(&DataKey::MinBond, &new_min_bond);
        bump_instance(&env);
    }

    /// Seed the reward pool. Transfers `amount` from admin and distributes
    /// over `duration` seconds using Synthetix reward-per-token.
    pub fn notify_reward_amount(
        env: Env,
        amount: i128,
        duration: u64,
    ) -> Result<(), ContractError> {
        let admin = get_admin(&env);
        admin.require_auth();

        if amount <= 0 || duration == 0 {
            return Err(ContractError::InvalidAmount);
        }

        update_rewards(&env, None);

        let now = env.ledger().timestamp();
        let end = get_reward_end(&env);

        let rate = if now >= end {
            amount / duration as i128
        } else {
            let remaining = (end - now) as i128;
            let leftover = remaining.checked_mul(get_reward_rate(&env)).unwrap();
            (amount + leftover) / duration as i128
        };

        if rate == 0 {
            return Err(ContractError::ZeroRewardRate);
        }

        // Transfer reward tokens into contract
        let tok = token::Client::new(&env, &get_token(&env));
        tok.transfer(&admin, &env.current_contract_address(), &amount);

        env.storage().instance().set(&DataKey::RewardRate, &rate);
        env.storage().instance().set(&DataKey::LastUpdateTime, &now);
        env.storage()
            .instance()
            .set(&DataKey::RewardEndTime, &(now + duration));

        bump_instance(&env);
        RewardEvent { amount, duration }.publish(&env);
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Read-only helpers
    // -----------------------------------------------------------------------

    pub fn admin(env: Env) -> Address {
        get_admin(&env)
    }

    pub fn token(env: Env) -> Address {
        get_token(&env)
    }

    pub fn total_bonds(env: Env) -> i128 {
        get_total_bonds(&env)
    }

    pub fn min_bond(env: Env) -> i128 {
        get_min_bond(&env)
    }

    /// Returns the bond amount for a service, or 0 if not registered.
    /// Used by Escalation contract for progressive slash computation.
    pub fn bond_of(env: Env, service: Address) -> i128 {
        bump_instance(&env);
        get_service(&env, &service)
            .map(|info| info.bond)
            .unwrap_or(0)
    }
}
