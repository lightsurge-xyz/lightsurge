use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ContractError {
    AlreadyRegistered = 1,
    NotRegistered = 2,
    InsufficientBond = 3,
    Unauthorized = 4,
    InvalidAmount = 5,
    ServiceNotActive = 6,
    CooldownNotPassed = 7,
    NoPendingUnstake = 8,
    PendingUnstakeExists = 9,
    BondBelowMinimum = 10,
    NothingToSlash = 11,
}
