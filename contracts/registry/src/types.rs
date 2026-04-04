use soroban_sdk::{contracttype, Address, BytesN};

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ServiceStatus {
    Active,
    Unbonding,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceInfo {
    pub owner: Address,
    pub metadata_hash: BytesN<32>,
    pub bond: i128,
    pub pending_unstake: i128,
    pub unstake_time: u64,
    pub reputation: u32,
    pub status: ServiceStatus,
    pub registered_at: u64,
    pub last_slash_time: u64,
    pub reward_per_token_paid: i128,
    pub accrued_rewards: i128,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    // Instance storage — global config
    Admin,
    Token,
    MinBond,
    CooldownPeriod,
    DisputeResolver,
    Treasury,
    // Instance storage — reward state
    RewardRate,
    RewardPerTokenStored,
    LastUpdateTime,
    RewardEndTime,
    TotalBonds,
    // Persistent storage — per-service
    Service(Address),
}
