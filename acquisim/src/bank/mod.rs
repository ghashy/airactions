use std::sync::Arc;

use secrecy::{ExposeSecret, Secret};
use serde::Serialize;
use time::{
    format_description::well_known::iso8601::TimePrecision, OffsetDateTime,
};
use tokio::sync::{Mutex, MutexGuard, TryLockError};

use crate::{
    domain::card_number::CardNumber, error_chain_fmt, middleware::Credentials,
};
use time::format_description::well_known::{iso8601, Iso8601};

const SIMPLE_ISO: Iso8601<6651332276402088934156738804825718784> = Iso8601::<
    {
        iso8601::Config::DEFAULT
            .set_year_is_six_digits(false)
            .set_time_precision(TimePrecision::Second {
                decimal_digits: None,
            })
            .encode()
    },
>;

time::serde::format_description!(iso_format, OffsetDateTime, SIMPLE_ISO);

#[derive(Clone)]
pub struct Bank(Arc<Mutex<BankInner>>);

#[derive(thiserror::Error)]
pub enum BankOperationError {
    #[error("No account")]
    AccountNotFound,
    #[error("Account was deleted")]
    AccountIsDeleted,
    #[error("Not enough funds for operation")]
    NotEnoughFunds,
    #[error("Account is not authorized")]
    NotAuthorized,
    #[error("Can't perform transaction")]
    BadTransaction,
    #[error("Mutex lock error: {0}")]
    MutexLockError(#[from] TryLockError),
}

impl std::fmt::Debug for BankOperationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        error_chain_fmt(self, f)
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct Transaction {
    sender: Account,
    recipient: Account,
    amount: i64,
    #[serde(with = "iso_format")]
    datetime: OffsetDateTime,
}

#[derive(Serialize, Clone, Debug)]
pub struct Account {
    card_number: CardNumber,
    #[serde(skip)]
    password: Secret<String>,
    is_existing: bool,
}

impl Account {
    pub fn card(&self) -> CardNumber {
        self.card_number.clone()
    }
}

impl std::cmp::PartialEq for Account {
    fn eq(&self, other: &Self) -> bool {
        self.card_number.eq(&other.card_number)
    }
}

struct BankInner {
    accounts: Vec<Account>,
    transactions: Vec<Transaction>,
    system_account: Account,
    bank_username: String,
}

impl Bank {
    async fn lock(&self) -> MutexGuard<BankInner> {
        self.0.lock().await
    }

    /// Constructor
    pub fn new(cashbox_pass: &Secret<String>, bank_username: &str) -> Self {
        let system_account = Account {
            card_number: CardNumber::generate(),
            password: cashbox_pass.clone(),
            is_existing: true,
        };
        let bank = BankInner {
            accounts: Vec::new(),
            system_account,
            transactions: Vec::new(),
            bank_username: bank_username.to_string(),
        };
        Bank(Arc::new(Mutex::new(bank)))
    }

    /// Validate system account credentials
    pub async fn authorize_system(
        &self,
        credentials: Credentials,
    ) -> Result<(), BankOperationError> {
        let guard = self.lock().await;
        let bank_username = &guard.bank_username;
        let password = guard.system_account.password.expose_secret();
        if bank_username.eq(&credentials.username)
            && password.eq(credentials.password.expose_secret())
        {
            Ok(())
        } else {
            Err(BankOperationError::NotAuthorized)
        }
    }

    /// Add a new account
    pub async fn add_account(&self, password: &Secret<String>) -> CardNumber {
        let mut guard = self.lock().await;
        let account = Account {
            card_number: CardNumber::generate(),
            is_existing: true,
            password: password.clone(),
        };
        guard.accounts.push(account.clone());
        account.card_number
    }

    /// Mark existing account as deleted
    pub async fn delete_account(
        &self,
        card: CardNumber,
    ) -> Result<(), BankOperationError> {
        let mut guard = self.lock().await;
        match guard
            .accounts
            .iter_mut()
            .find(|acc| acc.card_number.eq(&card))
        {
            Some(acc) => {
                acc.is_existing = false;
                Ok(())
            }
            None => Err(BankOperationError::AccountNotFound),
        }
    }

    /// Get Vec<Account>
    pub async fn list_accounts(
        &self,
    ) -> Vec<crate::domain::responses::system_api::Account> {
        let lock = self.lock().await;
        let mut accounts = Vec::new();
        for acc in lock.accounts.iter() {
            accounts.push(crate::domain::responses::system_api::Account {
                card_number: acc.card_number.clone(),
                balance: self.balance(&lock, acc),
                transactions: self.account_transactions(&lock, acc),
            })
        }
        accounts
    }

    pub async fn authorize_account(
        &self,
        card: &CardNumber,
        password: &Secret<String>,
    ) -> Result<Account, BankOperationError> {
        let account = self.find_account(card).await?;
        if !account
            .password
            .expose_secret()
            .eq(password.expose_secret())
        {
            Err(BankOperationError::NotAuthorized)
        } else {
            Ok(account.clone())
        }
    }

    pub async fn find_account(
        &self,
        card: &CardNumber,
    ) -> Result<Account, BankOperationError> {
        let guard = self.lock().await;
        let account = guard
            .accounts
            .iter()
            .find(|&acc| acc.card_number.eq(card))
            .ok_or(BankOperationError::AccountNotFound)?;
        if !account.is_existing {
            return Err(BankOperationError::AccountIsDeleted);
        }
        Ok(account.clone())
    }

    fn balance(
        &self,
        guard: &MutexGuard<'_, BankInner>,
        account: &Account,
    ) -> i64 {
        let balance = guard
            .transactions
            .iter()
            .filter(|&transaction| {
                transaction.sender.eq(&account)
                    || transaction.recipient.eq(&account)
            })
            .fold(0 as i64, |amount, transaction| {
                if transaction.sender.eq(&account) {
                    amount - transaction.amount
                } else {
                    amount + transaction.amount
                }
            });
        balance
    }

    fn account_transactions(
        &self,
        guard: &MutexGuard<'_, BankInner>,
        acc: &Account,
    ) -> Vec<Transaction> {
        guard
            .transactions
            .iter()
            .filter(|&transaction| {
                transaction.sender.eq(&acc) || transaction.recipient.eq(&acc)
            })
            .cloned()
            .collect()
    }

    pub async fn new_transaction(
        &self,
        sender: &Account,
        recipient: &Account,
        amount: i64,
    ) -> Result<(), BankOperationError> {
        if sender == recipient {
            return Err(BankOperationError::BadTransaction);
        }

        let mut guard = self.lock().await;
        if self.balance(&guard, sender) < amount {
            return Err(BankOperationError::NotEnoughFunds);
        }

        if amount <= 0 {
            return Err(BankOperationError::BadTransaction);
        }

        let transaction = Transaction {
            sender: sender.clone(),
            recipient: recipient.clone(),
            amount,
            datetime: OffsetDateTime::now_utc(),
        };

        guard.transactions.push(transaction);

        Ok(())
    }

    pub async fn open_credit(
        &self,
        card: CardNumber,
        amount: i64,
    ) -> Result<(), BankOperationError> {
        let account = self.find_account(&card).await?.clone();

        let mut guard = self.lock().await;
        let transaction = Transaction {
            sender: guard.system_account.clone(),
            recipient: account,
            amount,
            datetime: OffsetDateTime::now_utc(),
        };

        guard.transactions.push(transaction);

        Ok(())
    }

    pub async fn list_transactions(&self) -> Vec<Transaction> {
        self.lock().await.transactions.clone()
    }
}

#[cfg(test)]
mod tests {
    use rs_merkle::{Hasher, MerkleTree};
    use secrecy::Secret;
    use time::OffsetDateTime;

    use crate::{bank::SIMPLE_ISO, domain::card_number::CardNumber};

    use super::Transaction;

    #[test]
    fn testmeme() {
        let transaction = Transaction {
            sender: super::Account {
                card_number: CardNumber::generate(),
                password: Secret::new("abc".to_string()),
                is_existing: true,
            },
            recipient: super::Account {
                card_number: CardNumber::generate(),
                password: Secret::new("abc".to_string()),
                is_existing: true,
            },
            amount: 10,
            datetime: OffsetDateTime::now_utc(),
        };

        let abc = serde_json::to_string_pretty(&transaction).unwrap();
        println!("{abc}");

        let now = OffsetDateTime::now_utc();
        println!("Now: {}", now.format(&SIMPLE_ISO).unwrap());
    }

    #[test]
    #[ignore]
    fn learn_merkle_tree_on_practice() {
        use rs_merkle::algorithms::Sha256;

        let mut tree: MerkleTree<Sha256> = rs_merkle::MerkleTree::new();
        let mut leaves =
            vec![Sha256::hash("a".as_bytes()), Sha256::hash("b".as_bytes())];
        tree.append(&mut leaves);
        let root = tree.root().unwrap_or_default();
        println!("No leaves: {}", hex::encode(root));
        tree.commit();
        let root = tree.root().unwrap();
        println!("After commit, a, b leaves: {}", hex::encode(root));

        dbg!(tree.leaves());

        let mut leaves = vec![Sha256::hash("c".as_bytes())];
        tree.append(&mut leaves);
        let root = tree.root().unwrap_or_default();
        println!("Before commit with c: {}", hex::encode(root));
        tree.commit();
        let root = tree.root().unwrap();
        println!("After commit with c: {}", hex::encode(root));
    }
}
