use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ContractError {
    DisputeNotFound = 1,
    DisputeAlreadyResolved = 2,
    DefenseWindowActive = 3,
    Unauthorized = 4,
    InvalidAmount = 5,
    DefenseWindowExpired = 6,
}
