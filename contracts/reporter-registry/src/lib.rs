//! Reporter registry: decides which addresses may post reserve reports and
//! supply snapshots to the coverage ledger, and in which role.
#![no_std]

use plimsoll_types::{
    PendingUpgrade, ReporterInfo, Role, INSTANCE_EXTEND_TO, INSTANCE_THRESHOLD,
    PERSISTENT_EXTEND_TO, PERSISTENT_THRESHOLD, UPGRADE_DELAY_LEDGERS,
};
use soroban_sdk::{
    contract, contracterror, contractevent, contractimpl, contracttype, panic_with_error, Address,
    BytesN, ContractExecutable, Env, String, Vec,
};

const MAX_NAME_LEN: u32 = 64;

#[contracttype]
#[derive(Clone)]
enum DataKey {
    Admin,
    Reporter(Address),
    /// Every address with a `Reporter` entry, in the order it was first added.
    Reporters,
    PendingUpgrade,
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum RegistryError {
    ReporterNotFound = 1,
    InvalidName = 2,
    UpgradeNotProposed = 3,
    UpgradeDelayNotElapsed = 4,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReporterSet {
    #[topic]
    pub reporter: Address,
    pub role: Role,
    pub name: String,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReporterRevoked {
    #[topic]
    pub reporter: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdminTransferred {
    #[topic]
    pub new_admin: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpgradeProposed {
    #[topic]
    pub wasm_hash: BytesN<32>,
    pub executable_at_ledger: u32,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpgradeExecuted {
    #[topic]
    pub wasm_hash: BytesN<32>,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpgradeCancelled {
    #[topic]
    pub wasm_hash: BytesN<32>,
}

#[contract]
pub struct ReporterRegistry;

fn admin(env: &Env) -> Address {
    env.storage()
        .instance()
        .get(&DataKey::Admin)
        .expect("admin is set in the constructor")
}

fn bump_instance(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(INSTANCE_THRESHOLD, INSTANCE_EXTEND_TO);
}

/// Every address with a `Reporter` entry, in first-add order. The index keeps
/// `reporter_count` a walk over the addresses rather than a scan of storage.
fn reporter_index(env: &Env) -> Vec<Address> {
    env.storage()
        .persistent()
        .get(&DataKey::Reporters)
        .unwrap_or_else(|| Vec::new(env))
}

fn pending(env: &Env) -> Option<PendingUpgrade> {
    env.storage().instance().get(&DataKey::PendingUpgrade)
}

#[contractimpl]
impl ReporterRegistry {
    pub fn __constructor(env: Env, admin: Address) {
        env.storage().instance().set(&DataKey::Admin, &admin);
        bump_instance(&env);
    }

    pub fn admin(env: Env) -> Address {
        admin(&env)
    }

    /// Add a reporter, or change an existing reporter's role and name.
    pub fn set_reporter(env: Env, reporter: Address, role: Role, name: String) {
        admin(&env).require_auth();
        if name.is_empty() || name.len() > MAX_NAME_LEN {
            panic_with_error!(&env, RegistryError::InvalidName);
        }
        let key = DataKey::Reporter(reporter.clone());
        let added_at = env
            .storage()
            .persistent()
            .get::<_, ReporterInfo>(&key)
            .map(|existing| existing.added_at)
            .unwrap_or_else(|| env.ledger().timestamp());
        let info = ReporterInfo {
            role,
            name: name.clone(),
            added_at,
        };
        env.storage().persistent().set(&key, &info);
        env.storage()
            .persistent()
            .extend_ttl(&key, PERSISTENT_THRESHOLD, PERSISTENT_EXTEND_TO);
        // Index the address once: a role or name change updates the entry in
        // place and must not make `reporter_count` see it twice.
        let index_key = DataKey::Reporters;
        let mut index = reporter_index(&env);
        if !index.contains(&reporter) {
            index.push_back(reporter.clone());
            env.storage().persistent().set(&index_key, &index);
            env.storage()
                .persistent()
                .extend_ttl(&index_key, PERSISTENT_THRESHOLD, PERSISTENT_EXTEND_TO);
        }
        bump_instance(&env);
        ReporterSet {
            reporter,
            role,
            name,
        }
        .publish(&env);
    }

    pub fn revoke(env: Env, reporter: Address) {
        admin(&env).require_auth();
        let key = DataKey::Reporter(reporter.clone());
        if !env.storage().persistent().has(&key) {
            panic_with_error!(&env, RegistryError::ReporterNotFound);
        }
        env.storage().persistent().remove(&key);
        let index_key = DataKey::Reporters;
        let mut index = reporter_index(&env);
        let mut position = None;
        for (i, who) in index.iter().enumerate() {
            if who == reporter {
                position = Some(i as u32);
                break;
            }
        }
        if let Some(i) = position {
            // `i` came from the walk above, so the removal always succeeds.
            let _ = index.remove(i);
            if index.is_empty() {
                env.storage().persistent().remove(&index_key);
            } else {
                env.storage().persistent().set(&index_key, &index);
                env.storage()
                    .persistent()
                    .extend_ttl(&index_key, PERSISTENT_THRESHOLD, PERSISTENT_EXTEND_TO);
            }
        }
        bump_instance(&env);
        ReporterRevoked { reporter }.publish(&env);
    }

    pub fn role_of(env: Env, reporter: Address) -> Option<Role> {
        Self::get_reporter(env, reporter).map(|info| info.role)
    }

    /// How many reporters the registry currently holds.
    ///
    /// Counts only entries that still exist: `revoke` removes an address from
    /// the index as well as its `ReporterInfo`, so a revoked reporter is never
    /// counted, and an index entry whose info is gone (a lapsed entry the host
    /// has since dropped) is skipped rather than counted. The walk is read-only
    /// and does not extend any TTL.
    pub fn reporter_count(env: Env) -> u32 {
        let index = reporter_index(&env);
        let mut count = 0u32;
        for reporter in index.iter() {
            let key = DataKey::Reporter(reporter);
            if env.storage().persistent().has(&key) {
                count += 1;
            }
        }
        count
    }

    pub fn get_reporter(env: Env, reporter: Address) -> Option<ReporterInfo> {
        let key = DataKey::Reporter(reporter);
        let info = env.storage().persistent().get::<_, ReporterInfo>(&key);
        if info.is_some() {
            env.storage()
                .persistent()
                .extend_ttl(&key, PERSISTENT_THRESHOLD, PERSISTENT_EXTEND_TO);
        }
        info
    }

    pub fn transfer_admin(env: Env, new_admin: Address) {
        admin(&env).require_auth();
        new_admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        bump_instance(&env);
        AdminTransferred { new_admin }.publish(&env);
    }

    /// Propose a wasm hash for installation after the upgrade delay. A new
    /// proposal replaces any pending one and restarts the clock.
    pub fn propose_upgrade(env: Env, wasm_hash: BytesN<32>) {
        admin(&env).require_auth();
        let executable_at_ledger = env
            .ledger()
            .sequence()
            .saturating_add(UPGRADE_DELAY_LEDGERS);
        env.storage().instance().set(
            &DataKey::PendingUpgrade,
            &PendingUpgrade {
                wasm_hash: wasm_hash.clone(),
                executable_at_ledger,
            },
        );
        bump_instance(&env);
        UpgradeProposed {
            wasm_hash,
            executable_at_ledger,
        }
        .publish(&env);
    }

    /// Install the pending wasm once its delay has elapsed.
    pub fn execute_upgrade(env: Env) {
        admin(&env).require_auth();
        let proposed = match pending(&env) {
            Some(proposed) => proposed,
            None => panic_with_error!(&env, RegistryError::UpgradeNotProposed),
        };
        if env.ledger().sequence() < proposed.executable_at_ledger {
            panic_with_error!(&env, RegistryError::UpgradeDelayNotElapsed);
        }
        env.storage().instance().remove(&DataKey::PendingUpgrade);
        bump_instance(&env);
        UpgradeExecuted {
            wasm_hash: proposed.wasm_hash.clone(),
        }
        .publish(&env);
        env.deployer()
            .update_current_contract(ContractExecutable::Wasm(proposed.wasm_hash));
    }

    /// Cancel the pending upgrade.
    pub fn cancel_upgrade(env: Env) {
        admin(&env).require_auth();
        let proposed = match pending(&env) {
            Some(proposed) => proposed,
            None => panic_with_error!(&env, RegistryError::UpgradeNotProposed),
        };
        env.storage().instance().remove(&DataKey::PendingUpgrade);
        bump_instance(&env);
        UpgradeCancelled {
            wasm_hash: proposed.wasm_hash,
        }
        .publish(&env);
    }

    /// The pending upgrade, if any.
    pub fn pending_upgrade(env: Env) -> Option<PendingUpgrade> {
        let proposed = pending(&env);
        if proposed.is_some() {
            bump_instance(&env);
        }
        proposed
    }
}

#[cfg(test)]
mod test;
