use soroban_sdk::{contracttype, Address, Bytes, BytesN};

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DisputeStatus {
    Open,
    Resolved,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Verdict {
    ClientWins,
    ServiceWins,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DisputeInfo {
    pub id: u64,
    pub client: Address,
    pub service: Address,
    pub receipt_hash: BytesN<32>,
    pub encrypted_response: Bytes,
    pub dispute_bond: i128,
    pub filed_at: u64,
    pub defense_window: u64,
    pub status: DisputeStatus,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    // Instance storage — global config
    Admin,
    Registry,
    Token,
    EvaluatorPubKey,
    DisputeBond,
    DefenseWindow,
    NextDisputeId,
    // Persistent storage — per-dispute / per-service
    Dispute(u64),
    OffenseCount(Address),
}
