#![cfg_attr(not(feature = "std"), no_std)]

use frame_support::{decl_error, decl_event, decl_module, decl_storage, dispatch, ensure, storage::IterableStorageMap};
use frame_system::{self as system, ensure_signed};

use sp_std::vec::Vec;

use primitives::{
	fee,
	traits::{DirectTrade, Matcher, Resolver, TokenPool, AMM},
	AssetId, Balance, ExchangeIntention, IntentionId, IntentionType,
};
use sp_std::cmp;

use orml_traits::{MultiCurrency, MultiCurrencyExtended};

#[cfg(test)]
mod mock;

#[cfg(test)]
mod tests;

/// The pallet's configuration trait.
pub trait Trait: system::Trait {
	type Event: From<Event<Self>> + Into<<Self as system::Trait>::Event>;

	type TokenPool: TokenPool<Self::AccountId, AssetId>;

	type AMMTrader: AMM<Self::AccountId, AssetId, Balance>;

	type DirectTrader: DirectTrade<Self::AccountId, AssetId, Balance>;

	type IntentionMatcher: Matcher<Self::AccountId, ExchangeIntention<Self::AccountId, AssetId, Balance>>;

	type Resolver: Resolver<Self::AccountId, ExchangeIntention<Self::AccountId, AssetId, Balance>>;

	type Currency: MultiCurrencyExtended<Self::AccountId, CurrencyId = AssetId, Balance = Balance, Amount = i128>;
}

pub type Intention<T> = ExchangeIntention<<T as system::Trait>::AccountId, AssetId, Balance>;

// This pallet's storage items.
decl_storage! {
	trait Store for Module<T: Trait> as Exchange {
		ExchangeAssetsIntentionCount get(fn get_intentions_count): map hasher(blake2_128_concat) (AssetId, AssetId) => u32;
		ExchangeAssetsIntentions get(fn get_intentions): map hasher(blake2_128_concat) (AssetId, AssetId) => Vec<Intention<T>>;

		Nonce: u128; // Used as intention ids for now
	}
}

// The pallet's events
decl_event!(
	pub enum Event<T>
	where
		AccountId = <T as system::Trait>::AccountId,
	{
		IntentionRegistered(AccountId, AssetId, AssetId, Balance, IntentionType, IntentionId),

		InsufficientAssetBalanceEvent(
			AccountId,
			AssetId,
			Balance,
			IntentionType,
			IntentionId,
			dispatch::DispatchError,
		),
		AMMSellErrorEvent(
			AccountId,
			AssetId,
			Balance,
			IntentionType,
			IntentionId,
			dispatch::DispatchError,
		),
		AMMBuyErrorEvent(
			AccountId,
			AssetId,
			Balance,
			IntentionType,
			IntentionId,
			dispatch::DispatchError,
		),
	}
);

// The pallet's errors
decl_error! {
	pub enum Error for Module<T: Trait> {
		/// Value was None
		NoneValue,
		/// Value reached maximum and cannot be incremented further
		StorageOverflow,
		TokenPoolNotFound,
		InsufficientAssetBalance,
	}
}

decl_module! {
	/// The module declaration.
	pub struct Module<T: Trait> for enum Call where origin: T::Origin {

		type Error = Error<T>;

		fn deposit_event() = default;

		/// Add new intention for new block
		#[weight = 10_000] // TODO: check correct weight
		pub fn sell(origin, asset_sell: AssetId,
							asset_buy: AssetId,
							amount_sell: Balance,
							discount: bool)  -> dispatch::DispatchResult {
			let who = ensure_signed(origin)?;

			ensure!(
				T::TokenPool::exists(asset_sell, asset_buy),
				Error::<T>::TokenPoolNotFound
			);

			ensure!(
				T::Currency::free_balance(asset_sell, &who) >= amount_sell,
				Error::<T>::InsufficientAssetBalance
			);


			let intention = Intention::<T> {
					who: who.clone(),
					asset_sell: asset_sell,
					asset_buy: asset_buy,
					amount: amount_sell,
					discount: discount,
					sell_or_buy : IntentionType::SELL,
					intention_id: Nonce::get()
			};

			<ExchangeAssetsIntentions<T>>::append((intention.asset_sell, intention.asset_buy), intention.clone());

			let asset_1 = cmp::min(intention.asset_sell, intention.asset_buy);
			let asset_2 = cmp::max(intention.asset_sell, intention.asset_buy);

			ExchangeAssetsIntentionCount::mutate((asset_1,asset_2), |total| *total = *total + 1u32);

			Self::deposit_event(RawEvent::IntentionRegistered(who, asset_sell, asset_buy, amount_sell, IntentionType::SELL, intention.intention_id));

			Nonce::mutate(|n| *n += 1);

			Ok(())
		}

		/// Add new intention for new block
		#[weight = 10_000] // TODO: check correct weight
		pub fn buy(origin, asset_buy: AssetId,
							asset_sell: AssetId,
							amount: Balance,
							discount: bool)  -> dispatch::DispatchResult {
			let who = ensure_signed(origin)?;

			ensure!(
				T::TokenPool::exists(asset_sell, asset_buy),
				Error::<T>::TokenPoolNotFound
			);

			ensure!(
				T::Currency::free_balance(asset_sell, &who) >= amount,
				Error::<T>::InsufficientAssetBalance
			);

			let intention = Intention::<T> {
					who: who.clone(),
					asset_sell: asset_sell,
					asset_buy: asset_buy,
					amount: amount,
					sell_or_buy: IntentionType::BUY,
					discount: discount,
					intention_id: Nonce::get()
			};

			<ExchangeAssetsIntentions<T>>::append((intention.asset_sell, intention.asset_buy), intention.clone());

			let asset_1 = cmp::min(intention.asset_sell, intention.asset_buy);
			let asset_2 = cmp::max(intention.asset_sell, intention.asset_buy);

			ExchangeAssetsIntentionCount::mutate((asset_1,asset_2), |total| *total = *total + 1u32);

			Self::deposit_event(RawEvent::IntentionRegistered(who, asset_sell, asset_buy, amount, IntentionType::BUY, intention.intention_id));

			Nonce::mutate(|n| *n += 1);

			Ok(())
		}

		fn on_finalize(){

			for ((asset_1,asset_2), count) in ExchangeAssetsIntentionCount::iter() {
				if count == 0 {
					continue;
				}
				let pair_account = T::TokenPool::get_pair_id(&asset_1, &asset_2);

				let asset_a_sells = <ExchangeAssetsIntentions<T>>::get((asset_2, asset_1));
				let asset_b_sells = <ExchangeAssetsIntentions<T>>::get((asset_1, asset_2));

				Self::process_exchange_intentions(&pair_account, &asset_a_sells, &asset_b_sells);

			}

			ExchangeAssetsIntentionCount::remove_all();
			ExchangeAssetsIntentions::<T>::remove_all();
		}
	}
}

// "Internal" functions, callable by code.
impl<T: Trait> Module<T> {
	fn process_exchange_intentions(
		pair_account: &T::AccountId,
		sell_a_intentions: &Vec<Intention<T>>,
		sell_b_intentions: &Vec<Intention<T>>,
	) -> bool {
		T::IntentionMatcher::group(pair_account, sell_a_intentions, sell_b_intentions);
		true
	}

	fn amm_exchange(
		who: &T::AccountId,
		exchange_type: &IntentionType,
		intention_id: IntentionId,
		asset_sell: AssetId,
		asset_buy: AssetId,
		amount: Balance,
		discount: bool,
	) -> bool {
		match exchange_type {
			IntentionType::SELL => match T::AMMTrader::sell(who, asset_sell, asset_buy, amount, discount) {
				Ok(()) => true,
				Err(error) => {
					Self::deposit_event(RawEvent::AMMSellErrorEvent(
						who.clone(),
						asset_sell,
						amount,
						exchange_type.clone(),
						intention_id,
						error.into(),
					));
					false
				}
			},

			IntentionType::BUY => match T::AMMTrader::buy(who, asset_buy, asset_sell, amount, discount) {
				Ok(()) => true,
				Err(error) => {
					Self::deposit_event(RawEvent::AMMBuyErrorEvent(
						who.clone(),
						asset_buy,
						amount,
						exchange_type.clone(),
						intention_id,
						error.into(),
					));
					false
				}
			},
		}
	}
}

impl<T: Trait> Resolver<T::AccountId, ExchangeIntention<T::AccountId, AssetId, Balance>> for Module<T> {
	fn resolve_single_intention(intention: &ExchangeIntention<T::AccountId, AssetId, Balance>) {
		Self::amm_exchange(
			&intention.who,
			&intention.sell_or_buy,
			intention.intention_id,
			intention.asset_sell,
			intention.asset_buy,
			intention.amount,
			intention.discount,
		);
	}

	fn resolve_intention(
		pair_account: &T::AccountId,
		intention: &ExchangeIntention<T::AccountId, AssetId, Balance>,
		matched: &Vec<ExchangeIntention<T::AccountId, AssetId, Balance>>,
	) -> bool {
		let asset_a_reserve = T::Currency::free_balance(intention.asset_sell, pair_account);
		let asset_b_reserve = T::Currency::free_balance(intention.asset_buy, pair_account);

		let mut intention_copy = intention.clone();

		for matched_intention in matched.iter() {
			let amount_a = intention_copy.amount;
			let amount_b = matched_intention.amount;

			let spot_price_a = match T::AMMTrader::calculate_spot_price(asset_a_reserve, asset_b_reserve, amount_a) {
				Ok(price) => price,
				Err(_error) => {
					// Note : Should not happen if pool exists and is not 0 (should not happen because 0 value pools should be destroyed)
					return false;
				}
			};
			let spot_price_b = match T::AMMTrader::calculate_spot_price(asset_b_reserve, asset_a_reserve, amount_b) {
				Ok(price) => price,
				Err(_error) => {
					// Note : Should not happen if pool exists and is not 0 (should not happen because 0 value pools should be destroyed)
					return false;
				}
			};

			if amount_a > spot_price_b {
				if T::Currency::free_balance(intention.asset_sell, &intention.who) < spot_price_b {
					Self::deposit_event(RawEvent::InsufficientAssetBalanceEvent(
						intention.who.clone(),
						intention.asset_sell,
						spot_price_b,
						intention.sell_or_buy.clone(),
						intention.intention_id,
						Error::<T>::InsufficientAssetBalance.into(),
					));
					return false;
				}

				if T::Currency::free_balance(intention.asset_buy, &matched_intention.who) < amount_b {
					Self::deposit_event(RawEvent::InsufficientAssetBalanceEvent(
						matched_intention.who.clone(),
						intention.asset_buy,
						amount_b,
						matched_intention.sell_or_buy.clone(),
						matched_intention.intention_id,
						Error::<T>::InsufficientAssetBalance.into(),
					));
					return false;
				}

				intention_copy.amount = amount_a - spot_price_b;

				let transfer_a_fee = fee::get_fee(spot_price_b).unwrap();
				let transfer_b_fee = fee::get_fee(amount_b).unwrap();

				// If ok , do direct transfer - this should not fail at this point
				T::DirectTrader::transfer(
					&intention.who,
					&matched_intention.who,
					intention.asset_sell,
					spot_price_b - transfer_a_fee,
				)
				.expect("Should not failed. Checks had been done.");
				T::DirectTrader::transfer(
					&matched_intention.who,
					&intention.who,
					intention.asset_buy,
					amount_b - transfer_b_fee,
				)
				.expect("Should not failed. Checks had been done.");

				T::DirectTrader::transfer(&intention.who, &pair_account, intention.asset_sell, transfer_a_fee)
					.expect("Should not failed. Checks had been done.");

				T::DirectTrader::transfer(
					&matched_intention.who,
					&pair_account,
					intention.asset_buy,
					transfer_b_fee,
				)
				.expect("Should not failed. Checks had been done.");
			} else if amount_a < spot_price_b {
				if T::Currency::free_balance(intention.asset_sell, &intention.who) < amount_a {
					Self::deposit_event(RawEvent::InsufficientAssetBalanceEvent(
						intention.who.clone(),
						intention.asset_sell,
						spot_price_b,
						intention.sell_or_buy.clone(),
						intention.intention_id,
						Error::<T>::InsufficientAssetBalance.into(),
					));
					return false;
				}

				if T::Currency::free_balance(intention.asset_buy, &matched_intention.who) < spot_price_a {
					Self::deposit_event(RawEvent::InsufficientAssetBalanceEvent(
						matched_intention.who.clone(),
						intention.asset_buy,
						amount_b,
						matched_intention.sell_or_buy.clone(),
						matched_intention.intention_id,
						Error::<T>::InsufficientAssetBalance.into(),
					));
					return false;
				}

				let rest_amount = amount_b - spot_price_a;

				match Self::amm_exchange(
					&matched_intention.who,
					&matched_intention.sell_or_buy,
					matched_intention.intention_id,
					matched_intention.asset_sell,
					matched_intention.asset_buy,
					rest_amount,
					matched_intention.discount,
				) {
					true => {
						let transfer_a_fee = fee::get_fee(amount_a).unwrap();
						let transfer_b_fee = fee::get_fee(spot_price_a).unwrap();

						// If ok , do direct transfer - this should not fail at this point
						T::DirectTrader::transfer(
							&intention.who,
							&matched_intention.who,
							intention.asset_sell,
							amount_a - transfer_a_fee,
						)
						.expect("Should not failed. Checks had been done.");
						T::DirectTrader::transfer(
							&matched_intention.who,
							&intention.who,
							intention.asset_buy,
							spot_price_a - transfer_b_fee,
						)
						.expect("Should not failed. Checks had been done.");

						T::DirectTrader::transfer(&intention.who, &pair_account, intention.asset_sell, transfer_a_fee)
							.expect("Should not failed. Checks had been done.");

						T::DirectTrader::transfer(
							&matched_intention.who,
							&pair_account,
							intention.asset_buy,
							transfer_b_fee,
						)
						.expect("Should not failed. Checks had been done.");

						intention_copy.amount = 0;
					}
					false => {
						return false;
					}
				}
			} else {
				let transfer_a_fee = fee::get_fee(amount_a).unwrap();
				let transfer_b_fee = fee::get_fee(amount_b).unwrap();

				T::DirectTrader::transfer(
					&intention.who,
					&matched_intention.who,
					intention.asset_sell,
					amount_a - transfer_a_fee,
				)
				.expect("Should not failed. Checks had been done.");
				T::DirectTrader::transfer(
					&matched_intention.who,
					&intention.who,
					intention.asset_buy,
					amount_b - transfer_b_fee,
				)
				.expect("Should not failed. Checks had been done.");

				T::DirectTrader::transfer(&intention.who, &pair_account, intention.asset_sell, transfer_a_fee)
					.expect("Should not failed. Checks had been done.");

				T::DirectTrader::transfer(
					&matched_intention.who,
					&pair_account,
					intention.asset_buy,
					transfer_b_fee,
				)
				.expect("Should not failed. Checks had been done.");

				intention_copy.amount = 0;
			}
		}

		// If there is something left, just resolve as single intention
		if intention_copy.amount > 0 {
			Self::resolve_single_intention(&intention_copy);
		}

		true
	}
}

impl<T: Trait> DirectTrade<T::AccountId, AssetId, Balance> for Module<T> {
	fn transfer(from: &T::AccountId, to: &T::AccountId, asset: u32, amount: u128) -> dispatch::DispatchResult {
		T::Currency::transfer(asset, from, &to, amount)
	}
}

impl<T: Trait> Matcher<T::AccountId, ExchangeIntention<T::AccountId, AssetId, Balance>> for Module<T> {
	fn group<'a>(
		pair_account: &T::AccountId,
		asset_a_sell: &'a Vec<ExchangeIntention<T::AccountId, AssetId, Balance>>,
		asset_b_sell: &'a Vec<ExchangeIntention<T::AccountId, AssetId, Balance>>,
	) -> Option<
		Vec<(
			ExchangeIntention<T::AccountId, AssetId, Balance>,
			Vec<ExchangeIntention<T::AccountId, AssetId, Balance>>,
		)>,
	> {
		let mut b_copy = asset_b_sell.clone();
		let mut a_copy = asset_a_sell.clone();

		b_copy.sort_by(|a, b| b.amount.cmp(&a.amount));
		a_copy.sort_by(|a, b| b.amount.cmp(&a.amount));

		for intention in a_copy {
			let mut bvec = Vec::<Intention<T>>::new();
			let mut total = 0;
			let mut idx: usize = 0;

			// we can further optimize this loop!
			loop {
				let m = match b_copy.get(idx) {
					Some(x) => x,
					None => break,
				};

				if m.amount + total <= intention.amount {
					bvec.push(m.clone());
					total += m.amount;
					b_copy.remove(idx);
				}

				idx += 1;
			}

			T::Resolver::resolve_intention(pair_account, &intention, &bvec);
		}

		while let Some(b_intention) = b_copy.pop() {
			T::Resolver::resolve_single_intention(&b_intention);
		}

		None
	}
}