/// A runtime module template with necessary imports

/// Feel free to remove or edit this file as needed.
/// If you change the name of this file, make sure to update its references in runtime/src/lib.rs
/// If you remove this file, you can remove those references


/// For more guidance on Substrate modules, see the example module
/// https://github.com/paritytech/substrate/blob/master/srml/example/src/lib.rs

use support::{decl_module, decl_storage, decl_event, ensure, StorageMap, dispatch::Result, traits::{ReservableCurrency}};
use runtime_primitives::traits::{Hash, Verify, CheckedAdd, CheckedSub, As};
use system::{ensure_signed};
use parity_codec::{Encode, Decode};
use primitives::{sr25519::{Public, Signature}};

/// Length of the pending_window in seconds
const PENDING_WINDOW: u64 = 1 * 60;

#[derive(Clone, PartialEq, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug))]
pub struct ChannelBalance<Balance> {
	balance: Balance,
	balance_a: Balance,
}

#[derive(Clone, PartialEq, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug))]
pub enum Channel<Balance, Moment> {
	Uninitialized,
	Funded(ChannelBalance<Balance>),
	Active(ChannelBalance<Balance>),
	PendingSettlement(ChannelBalance<Balance>, Moment)
}

impl<Balance, Moment> Default for Channel<Balance, Moment> {
	fn default() -> Self { Self::Uninitialized }
}

#[derive(Encode, Decode, Default, Clone, PartialEq)]
#[cfg_attr(feature = "std", derive(Debug))]
pub struct State<Hash> {
	// number of open channels
	// Note: the smart contract doesn't know the actual
	//       channels but it knows how many open ones
	//       there are.
	// openChannels: u16,
	secret: Hash
}

pub type ChannelId<T> = <T as system::Trait>::Hash;
pub type PreImage<T> = <T as system::Trait>::Hash;

/// The module's configuration trait.
pub trait Trait: system::Trait + timestamp::Trait + balances::Trait {
	/// The overarching event type.
	type Event: From<Event<Self>> + Into<<Self as system::Trait>::Event>;
}

decl_storage! {
	trait Store for Module<T: Trait> as hopr {
		Channels get(channels): map ChannelId<T> => Channel<T::Balance, T::Moment>;
		States get(state): map T::AccountId => State<T::Hash>;
		Nonces get(nonce_exists): map T::Hash => bool;
		AccountIdMap get(pubkey): map T::AccountId => Public;
	}
}

decl_module! {
	/// The module declaration.
	pub struct Module<T: Trait> for enum Call where origin: T::Origin {
		fn deposit_event<T>() = default;

		/// Initialises a payment channel between two parties.
		pub fn create(origin, funds: T::Balance, counterparty: T::AccountId) -> Result {
			// ==== Verification ================================
			let sender = ensure_signed(origin)?;

			ensure!(<balances::Module<T> as ReservableCurrency<<T as system::Trait>::AccountId>>::can_reserve(&sender, funds), "User has not enough funds.");

			let channel_id = Self::get_id(&sender, &counterparty);

			let mut channel = Self::channels(channel_id);

			match channel {
				Channel::Uninitialized => {
					if Self::is_party_a(&sender, &counterparty) {
						channel = Channel::Funded(ChannelBalance {
							balance: funds,
							balance_a: funds,
						});
					} else {
						channel = Channel::Funded(ChannelBalance {
							balance: funds,
							balance_a: <T::Balance as As<u64>>::sa(0),
						});
					}
				},
				Channel::Funded(channel_balance) => {
					if Self::is_party_a(&sender, &counterparty) {
						channel = Channel::Funded(ChannelBalance {
							balance: channel_balance.balance.checked_add(&funds).ok_or("integer error")?,
							balance_a: channel_balance.balance_a.checked_add(&funds).ok_or("integer error")?,
						});
					} else {
						channel = Channel::Funded(ChannelBalance {
							balance: channel_balance.balance.checked_add(&funds).ok_or("integer error")?,
							balance_a: channel_balance.balance_a.checked_sub(&funds).ok_or("integer error")?,
						});
					}
				},
				_ => panic!("Channel is cannot be created twice."),
			}

			// ==== State change ================================
			<Channels<T>>::insert(channel_id, channel);
			<balances::Module<T> as ReservableCurrency<<T as system::Trait>::AccountId>>::reserve(&sender, funds)?;
			
			Ok(())
		}

		/// Turns a previously funded channel into an active one.
		pub fn set_active(origin, counterparty: T::AccountId, signature: Signature) -> Result {
			// ==== Verification ================================
			let sender = ensure_signed(origin)?;

			ensure!(<AccountIdMap<T>>::exists(&counterparty), "We do not know the public key of the counterparty.");
			
			let channel_id = Self::get_id(&sender, &counterparty);

			let mut channel = Self::channels(&channel_id);

			let counterparty_pubkey = Self::pubkey(counterparty);

			let channel_balance: Option<ChannelBalance<T::Balance>>;

			ensure!(Signature::verify(&signature, channel.encode().as_slice(), &counterparty_pubkey), "Invalid signature.");
		
			if let Channel::Funded(_channel_balance) = channel {
				channel_balance = Some(_channel_balance.clone());
				channel = Channel::Active(_channel_balance);
			} else {
				panic!("Channel does not exist and/or its state does not fit.")
			}

			// ==== State change ================================
			Self::test_and_set_nonce(<T as system::Trait>::Hashing::hash(signature.as_ref()))?;

			<Channels<T>>::insert(channel_id, channel);

			Self::deposit_event(RawEvent::Opened(channel_id, channel_balance.clone().unwrap().balance, channel_balance.unwrap().balance_a));

			Ok(())
		}

		/// Initialise a channel that is funded by both sides and turn it immediately into
		/// an active one.
		pub fn create_funded(origin, counterparty: T::AccountId, signature: Signature, funds: T::Balance) -> Result {
			// ==== Verification ================================
			let sender = ensure_signed(origin)?;

			ensure!(<AccountIdMap<T>>::exists(&counterparty), "We do not know the public key of the counterparty.");

			let channel_balance = ChannelBalance {
				balance: funds.checked_add(&funds).ok_or("integer error")?,
				balance_a: funds,
			};

			let mut channel = Channel::Funded(channel_balance.clone());

			let counterparty_pubkey: Public = Self::pubkey(&counterparty);

			let channel_id: ChannelId<T> = Self::get_id(&counterparty, &sender);

			ensure!(!<Channels<T>>::exists(&channel_id), "Channel must not exist.");

			ensure!(Signature::verify(&signature, channel.encode().as_slice(), &counterparty_pubkey), "Signature must be valid.");

			ensure!(<balances::Module<T> as ReservableCurrency<<T as system::Trait>::AccountId>>::can_reserve(&sender, funds), "User does have not enough funds.");
			ensure!(<balances::Module<T> as ReservableCurrency<<T as system::Trait>::AccountId>>::can_reserve(&counterparty, funds), "Counterparty does not have enough funds.");

			channel = Channel::Active(channel_balance.clone());

			// ==== State change ================================
			Self::test_and_set_nonce(<T as system::Trait>::Hashing::hash(signature.as_ref()))?;

			<balances::Module<T> as ReservableCurrency<<T as system::Trait>::AccountId>>::reserve(&sender, funds)?;
			<balances::Module<T> as ReservableCurrency<<T as system::Trait>::AccountId>>::reserve(&counterparty, funds)?;

			<Channels<T>>::insert(channel_id, channel);

			Self::deposit_event(RawEvent::Opened(channel_id, channel_balance.balance, channel_balance.balance_a));

			Ok(())
		}

		/// Resets the stored on-chain secret.
		pub fn set_secret(origin, hash: T::Hash) {
			// ==== Verification ================================
			let sender = ensure_signed(origin)?;

			// ==== State change ================================
			<States<T>>::mutate(&sender, |state| {
				state.secret = hash;
			})
		}

		pub fn redeem_ticket(origin, signature: Signature, counterparty: T::AccountId, pre_image: PreImage<T>, s_a: PreImage<T>, s_b: PreImage<T>, amount: T::Balance, win_prob: T::Hash) -> Result {
			// ==== Verification ================================
			let sender = ensure_signed(origin)?;

			ensure!(<States<T>>::exists(&sender), "Sender must have set an on-chain secret.");
			ensure!(<T as system::Trait>::Hashing::hash(pre_image.as_ref()) == Self::state(&sender).secret, "Given value is not a pre-image of the stored on-chain secret");

			ensure!(<AccountIdMap<T>>::exists(&counterparty), "We do not know the public key of the counterparty.");

			let counterparty_pubkey = Self::pubkey(&counterparty);

			let channel_id = Self::get_id(&sender, &counterparty);
			let channel = Self::channels(channel_id);
			let mut _channel_balance: Option<ChannelBalance<T::Balance>> = None;

			match channel.clone() {
				Channel::Active(__channel_balance) => {
					_channel_balance = Some(__channel_balance);
				},
				Channel::PendingSettlement(__channel_balance, timestamp) => {
					if timestamp::Module::<T>::now() > timestamp {
						panic!("Ticket redemption must have happened before end of pending window.")
					}
					_channel_balance = Some(__channel_balance);
				},
				_ => panic!("Channel does not exist and/or its state does not fit.")
			}

			let mut updated_balance = _channel_balance.unwrap();

			if Self::is_party_a(&sender, &counterparty) {
				ensure!(updated_balance.balance_a.checked_add(&amount).ok_or("Integer error.")? <= updated_balance.balance, "Transferred funds must not exceed channel balance.")
			} else {
				ensure!(updated_balance.balance_a.checked_sub(&amount).ok_or("Integer error.")? >= <<T as balances::Trait>::Balance as As<u64>>::sa(0), "Transferred funds must not exceed channel balance.")
			}

			let hashed_s_a = <T as system::Trait>::Hashing::hash(s_a.as_ref());
			let hashed_s_b = <T as system::Trait>::Hashing::hash(s_b.as_ref());

			let challenge = (hashed_s_a, hashed_s_b).using_encoded(<T as system::Trait>::Hashing::hash);

			let ticket = (challenge, pre_image, amount, win_prob);
			let hashed_ticket = ticket.using_encoded(<T as system::Trait>::Hashing::hash);

			ensure!(Self::cmp_hash(&hashed_ticket, &win_prob), "Ticket must be a win.");

			ensure!(Verify::verify(&signature, ticket.encode().as_slice(), &counterparty_pubkey), "Signature must be valid.");

			// ==== Prepare state change=========================
			if Self::is_party_a(&sender, &counterparty) {
				updated_balance.balance_a = updated_balance.balance_a.checked_add(&amount).ok_or("Integer error.")?;
			} else {
				updated_balance.balance_a = updated_balance.balance_a.checked_sub(&amount).ok_or("Integer error.")?; 
			}

			// ==== State change ================================
			Self::test_and_set_nonce(<T as system::Trait>::Hashing::hash(signature.as_ref()))?;
			<States<T>>::mutate(&sender, |state| {
				state.secret = pre_image;
			});

			match channel {
				Channel::Active(_) => <Channels<T>>::insert(&channel_id, Channel::Active(updated_balance)),
				Channel::PendingSettlement(_, timestamp) => <Channels<T>>::insert(&channel_id, Channel::PendingSettlement(updated_balance, timestamp)),
				_ => panic!("Channel does not exist and/or its state does not fit.")
			};

			Ok(())
		}

		pub fn initiate_recovery() {
			// TODO
		}

		pub fn initiate_settlement(origin, counterparty: T::AccountId) {
			// ==== Verification ================================
			let sender = ensure_signed(origin)?;

			ensure!(<AccountIdMap<T>>::exists(&counterparty), "We do not know the public key of the counterparty.");

			let channel_id = Self::get_id(&sender, &counterparty);

			let channel = Self::channels(channel_id);
			let mut _channel_balance: Option<ChannelBalance<T::Balance>> = None;

			match channel.clone() {
				Channel::Active(__channel_balance) => {
					_channel_balance = Some(__channel_balance);
				},
				_ => panic!("Channel does not exist and/or its state does not fit.")
			}

			// ==== State change ================================
			let end_of_pending_window = timestamp::Module::<T>::now().checked_add(&<T::Moment as As<u64>>::sa(PENDING_WINDOW)).ok_or("Integer overflow")?;
			<Channels<T>>::insert(channel_id, Channel::PendingSettlement(_channel_balance.clone().unwrap(), end_of_pending_window));

			Self::deposit_event(RawEvent::InitiatedSettlement(channel_id, _channel_balance.unwrap().balance));
		}

		pub fn withdraw() {
			// TODO
		}
	}
}

decl_event!(
	pub enum Event<T> where 
		<T as system::Trait>::AccountId,
		<T as system::Trait>::Hash,
		<T as balances::Trait>::Balance {
		Opened(Hash, Balance, Balance),
		InitiatedSettlement(Hash, Balance),
		OpenedFor(AccountId, AccountId, Balance, Balance),
	}
);

impl<T: Trait> Module<T> {
	fn test_and_set_nonce(nonce: T::Hash) -> Result {
		ensure!(!<Nonces<T>>::exists(nonce), "Nonce was already used.");
		<Nonces<T>>::insert(nonce, true);
		Ok(())
	}

	fn is_party_a(a: &T::AccountId, b: &T::AccountId) -> bool {
		a < b
	}

	/// Give the payment channels a meaningful ID that is the same for
	/// parties
	fn get_id(a: &T::AccountId, b: &T::AccountId) -> ChannelId<T> {
		if Self::is_party_a(&a, &b) {
			(a, b).using_encoded(<T as system::Trait>::Hashing::hash)
		} else {
			(b, a).using_encoded(<T as system::Trait>::Hashing::hash)
		}
	}

	fn cmp_hash(first_hash: &T::Hash, second_hash: &T::Hash) -> bool {
		*(first_hash.as_ref()) < *(second_hash.as_ref())
	}
}

/// tests for this module
#[cfg(test)]
mod tests {
	use super::*;

	use runtime_io::with_externalities;
	use primitives::{H256, Blake2Hasher};
	use support::{impl_outer_origin, assert_ok};
	use runtime_primitives::{
		BuildStorage,
		traits::{BlakeTwo256, IdentityLookup},
		testing::{Digest, DigestItem, Header}
	};

	impl_outer_origin! {
		pub enum Origin for Test {}
	}

	// For testing the module, we construct most of a mock runtime. This means
	// first constructing a configuration type (`Test`) which `impl`s each of the
	// configuration traits of modules we want to use.
	#[derive(Clone, Eq, PartialEq)]
	pub struct Test;
	impl system::Trait for Test {
		type Origin = Origin;
		type Index = u64;
		type BlockNumber = u64;
		type Hash = H256;
		type Hashing = BlakeTwo256;
		type Digest = Digest;
		type AccountId = u64;
		type Lookup = IdentityLookup<Self::AccountId>;
		type Header = Header;
		type Event = ();
		type Log = DigestItem;
	}
	impl Trait for Test {
		type Event = ();
	}
	type hopr = Module<Test>;

	// This function basically just builds a genesis storage key/value store according to
	// our desired mockup.
	fn new_test_ext() -> runtime_io::TestExternalities<Blake2Hasher> {
		system::GenesisConfig::<Test>::default().build_storage().unwrap().0.into()
	}

	#[test]
	fn it_works_for_default_value() {
		with_externalities(&mut new_test_ext(), || {
			// Just a dummy test for the dummy funtion `do_something`
			// calling the `do_something` function with a value 42
			assert_ok!(hopr::do_something(Origin::signed(1), 42));
			// asserting that the stored value is equal to what we stored
			assert_eq!(hopr::something(), Some(42));
		});
	}
}
