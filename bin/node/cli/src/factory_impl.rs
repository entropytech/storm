// Copyright 2019-2020 Parity Technologies (UK) Ltd.
// This file is part of Substrate.

// Substrate is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Substrate is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Substrate.  If not, see <http://www.gnu.org/licenses/>.

//! Implementation of the transaction factory trait, which enables
//! using the cli to manufacture transactions and distribute them
//! to accounts.

use rand::{Rng, SeedableRng};
use rand::rngs::StdRng;

use codec::{Encode, Decode};
use sp_keyring::sr25519::Keyring;
use node_runtime::{
	Call, CheckedExtrinsic, UncheckedExtrinsic, SignedExtra, Header,
	BalancesCall, NicksCall, AuthorshipCall, StakingCall,
	MinimumPeriod, ExistentialDeposit,
};
use node_primitives::Signature;
use sp_core::{sr25519, crypto::Pair, H256};
use sp_runtime::{
	generic::Era, Perbill,
	traits::{
		Block as BlockT, Header as HeaderT, SignedExtension, Verify, IdentifyAccount,
	}
};
use node_transaction_factory::RuntimeAdapter;
use sp_inherents::InherentData;
use sp_timestamp;
use sp_finality_tracker;

use pallet_staking::{RewardDestination, ValidatorPrefs};

type AccountPublic = <Signature as Verify>::Signer;

pub struct FactoryState<N> {
	tx_name: String,
	block_no: N,
	start_number: u32,
	round: u32,
	block_in_round: u32,
	num: u32,
	index: u32,
}

type Number = <<node_primitives::Block as BlockT>::Header as HeaderT>::Number;

impl<Number> FactoryState<Number> {
	fn build_extra(index: node_primitives::Index, phase: u64) -> node_runtime::SignedExtra {
		(
			frame_system::CheckVersion::new(),
			frame_system::CheckGenesis::new(),
			frame_system::CheckEra::from(Era::mortal(256, phase)),
			frame_system::CheckNonce::from(index),
			frame_system::CheckWeight::new(),
			pallet_transaction_payment::ChargeTransactionPayment::from(0),
			Default::default(),
		)
	}
}

impl RuntimeAdapter for FactoryState<Number> {
	type AccountId = node_primitives::AccountId;
	type Balance = node_primitives::Balance;
	type Block = node_primitives::Block;
	type Phase = sp_runtime::generic::Phase;
	type Secret = sr25519::Pair;
	type Index = node_primitives::Index;

	type Number = Number;

	fn new(
		tx_name: String,
		num: u64,
	) -> FactoryState<Self::Number> {
		FactoryState {
			tx_name,
			num: num as u32,
			round: 0,
			block_in_round: 0,
			block_no: 0,
			start_number: 0,
			index: 0,
		}
	}

	fn index(&self) -> u32 {
		self.index
	}

	fn increase_index(&mut self) {
		self.index += 1;
	}

	fn block_no(&self) -> Self::Number {
		self.block_no
	}

	fn block_in_round(&self) -> Self::Number {
		self.block_in_round
	}

	fn num(&self) -> Self::Number {
		self.num
	}

	fn round(&self) -> Self::Number {
		self.round
	}

	fn start_number(&self) -> Self::Number {
		self.start_number
	}

	fn set_block_no(&mut self, val: Self::Number) {
		self.block_no = val;
	}

	fn set_block_in_round(&mut self, val: Self::Number) {
		self.block_in_round = val;
	}

	fn set_round(&mut self, val: Self::Number) {
		self.round = val;
	}

	fn create_extrinsic(
		&mut self,
		sender: &Self::AccountId,
		key: &Self::Secret,
		destination: &Self::AccountId,
		amount: &Self::Balance,
		version: u32,
		genesis_hash: &<Self::Block as BlockT>::Hash,
		prior_block_hash: &<Self::Block as BlockT>::Hash,
	) -> <Self::Block as BlockT>::Extrinsic {
		println!("Creating a {} extrinsic...", self.tx_name);

		let phase = self.extract_phase(*prior_block_hash);

		let function = match self.tx_name.as_str() {
			"balances_transfer" => Call::Balances(BalancesCall::transfer(
				pallet_indices::address::Address::Id(destination.clone().into()),
				(*amount).into()
			)),
			"nicks_set_name" => Call::Nicks(NicksCall::set_name(
				b"Marcio Oscar Diaz".to_vec()
			)),
			"nicks_clear_name" => Call::Nicks(NicksCall::clear_name()),
			"authorship_set_uncles" => {
				let mut uncles = vec![];
				let num_headers = 10; // TODO: make it configurable.
				for _ in 0..num_headers {
					let header = Header::new(
						std::cmp::max(1, self.block_no()),
						H256::random(),
						H256::random(),
						genesis_hash.clone(),
						Default::default(),
					);
					uncles.push(header.clone());
				}
				Call::Authorship(AuthorshipCall::set_uncles(uncles))
			},
			"staking_bond" => {
				self.tx_name = String::from("staking_validate");
				Call::Staking(StakingCall::bond(
					pallet_indices::address::Address::Id(destination.clone().into()),
					(*amount).into(),
					RewardDestination::Controller,
				))
			},
			"staking_validate" => {
				self.tx_name = String::from("staking_nominate");
				Call::Staking(StakingCall::validate(
					ValidatorPrefs { commission: Perbill::from_rational_approximation(1u32, 10u32) }
				))
			},
			"staking_nominate" => {
				self.tx_name = String::from("staking_bond_extra");
				let mut targets = vec![];
				for _ in 0..16 {
					targets.push(pallet_indices::address::Address::Id(destination.clone().into()));
				}
				Call::Staking(StakingCall::nominate(targets))
			},
			"staking_bond_extra" => {
				self.tx_name = String::from("staking_unbond");
				Call::Staking(StakingCall::bond_extra(
					Self::minimum_balance() * 2,
				))
			},
			"staking_unbond" => { // TODO: Need to execute many of these guys to bump rebond.
				self.tx_name = String::from("staking_rebond");
				Call::Staking(StakingCall::unbond(
					Self::minimum_balance() * 2,
				))
			},
			"staking_rebond" => {
				self.tx_name = String::from("staking_withdraw_unbonded");
				Call::Staking(StakingCall::rebond(
					Self::minimum_balance(),
				))
			},
			"staking_withdraw_unbonded" => {
				self.tx_name = String::from("staking_bond_extra");
				Call::Staking(StakingCall::withdraw_unbonded())
			},
			other => panic!("Extrinsic {} is not supported yet!", other),
		};

		sign::<Self>(
			CheckedExtrinsic {
				signed: Some((sender.clone(), Self::build_extra(self.index, phase))),
				function,
			},
			key,
			(version, genesis_hash.clone(), prior_block_hash.clone(), (), (), (), ()),
		)
	}

	fn inherent_extrinsics(&self) -> InherentData {
		let timestamp = (self.block_no as u64 + 1) * MinimumPeriod::get();

		let mut inherent = InherentData::new();
		inherent.put_data(sp_timestamp::INHERENT_IDENTIFIER, &timestamp)
			.expect("Failed putting timestamp inherent");
		inherent.put_data(sp_finality_tracker::INHERENT_IDENTIFIER, &self.block_no)
			.expect("Failed putting finalized number inherent");
		inherent
	}

	fn minimum_balance() -> Self::Balance {
		ExistentialDeposit::get()
	}

	fn master_account_id() -> Self::AccountId {
		Keyring::Alice.to_account_id()
	}

	fn master_account_secret() -> Self::Secret {
		Keyring::Alice.pair()
	}

	/// Generates a random `AccountId` from `seed`.
	fn gen_random_account_id(seed: &Self::Number) -> Self::AccountId {
		let pair: sr25519::Pair = sr25519::Pair::from_seed(&gen_seed_bytes(*seed));
		AccountPublic::from(pair.public()).into_account()
	}

	/// Generates a random `Secret` from `seed`.
	fn gen_random_account_secret(seed: &Self::Number) -> Self::Secret {
		let pair: sr25519::Pair = sr25519::Pair::from_seed(&gen_seed_bytes(*seed));
		pair
	}

	fn extract_index(
		&self,
		_account_id: &Self::AccountId,
		_block_hash: &<Self::Block as BlockT>::Hash,
	) -> Self::Index {
		0
	}

	fn extract_phase(
		&self,
		_block_hash: <Self::Block as BlockT>::Hash
	) -> Self::Phase {
		// TODO get correct phase via api. See #2587.
		// This currently prevents the factory from being used
		// without a preceding purge of the database.
		self.block_no() as Self::Phase
	}
}

fn gen_seed_bytes(seed: u32) -> [u8; 32] {
	let mut rng: StdRng = SeedableRng::seed_from_u64(seed as u64);

	let mut seed_bytes = [0u8; 32];
	for i in 0..32 {
		seed_bytes[i] = rng.gen::<u8>();
	}
	seed_bytes
}

/// Creates an `UncheckedExtrinsic` containing the appropriate signature for
/// a `CheckedExtrinsics`.
fn sign<RA: RuntimeAdapter>(
	xt: CheckedExtrinsic,
	key: &sr25519::Pair,
	additional_signed: <SignedExtra as SignedExtension>::AdditionalSigned,
) -> <RA::Block as BlockT>::Extrinsic {
	let s = match xt.signed {
		Some((signed, extra)) => {
			let payload = (xt.function, extra.clone(), additional_signed);
			let signature = payload.using_encoded(|b| {
				if b.len() > 256 {
					key.sign(&sp_io::hashing::blake2_256(b))
				} else {
					key.sign(b)
				}
			}).into();
			UncheckedExtrinsic {
				signature: Some((pallet_indices::address::Address::Id(signed), signature, extra)),
				function: payload.0,
			}
		}
		None => UncheckedExtrinsic {
			signature: None,
			function: xt.function,
		},
	};

	let e = Encode::encode(&s);
	Decode::decode(&mut &e[..]).expect("Failed to decode signed unchecked extrinsic")
}
