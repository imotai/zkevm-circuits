use crate::gates::gate_helpers::{BlockCount2, Lane};
use crate::gates::running_sum::{
    BlockCountFinalConfig, LaneRotateConversionConfig,
};

use halo2::{
    circuit::{Layouter, Region},
    plonk::{Advice, Column, ConstraintSystem, Error},
};
use itertools::Itertools;
use pasta_curves::arithmetic::FieldExt;
use std::convert::TryInto;

#[derive(Clone)]
pub struct RhoConfig<F> {
    state: [Column<Advice>; 25],
    state_rotate_convert_configs: [LaneRotateConversionConfig<F>; 25],
    final_block_count_config: BlockCountFinalConfig<F>,
}

impl<F: FieldExt> RhoConfig<F> {
    pub fn configure(
        meta: &mut ConstraintSystem<F>,
        state: [Column<Advice>; 25],
    ) -> Self {
        let state_rotate_convert_configs = (0..5)
            .cartesian_product(0..5)
            .map(|(x, y)| LaneRotateConversionConfig::configure(meta, (x, y)))
            .collect::<Vec<_>>()
            .try_into()
            .unwrap();
        let final_block_count_config = BlockCountFinalConfig::configure(meta);
        Self {
            state,
            state_rotate_convert_configs,
            final_block_count_config,
        }
    }
    pub fn assign_rotation_checks(
        &self,
        layouter: &mut impl Layouter<F>,
        previous_state: [Lane<F>; 25],
    ) -> Result<[Lane<F>; 25], Error> {
        let lane_and_bcs: [(Lane<F>, BlockCount2<F>); 25] = previous_state
            .iter()
            .enumerate()
            .map(|(idx, lane)| {
                let (lane_next_row, bc) = &self.state_rotate_convert_configs
                    [idx]
                    .assign_region(
                        &mut layouter.namespace(|| format!("lane {}", idx)),
                        lane,
                    )
                    .unwrap();
                (lane_next_row.clone(), *bc)
            })
            .collect::<Vec<_>>()
            .try_into()
            .unwrap();
        let block_counts = lane_and_bcs.clone().map(|(_, bc)| bc);
        let next_state = lane_and_bcs.map(|(lane_next_row, _)| lane_next_row);

        self.final_block_count_config.assign_region(
            &mut layouter.namespace(|| "Final block count check"),
            block_counts,
        )?;
        Ok(next_state)
    }

    pub fn assign_region(
        &self,
        region: &mut Region<'_, F>,
        offset: usize,
        next_state: [Lane<F>; 25],
    ) -> Result<(), Error> {
        for (idx, next_lane) in next_state.iter().enumerate() {
            let cell = region.assign_advice(
                || "lane next row",
                self.state[idx],
                offset + 1,
                || Ok(next_lane.value),
            )?;
            region.constrain_equal(cell, next_lane.cell)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arith_helpers::*;
    use crate::common::*;
    use crate::gates::gate_helpers::*;
    use crate::keccak_arith::*;
    use halo2::circuit::Layouter;
    use halo2::plonk::{Advice, Column, ConstraintSystem, Error};
    use halo2::{circuit::SimpleFloorPlanner, dev::MockProver, plonk::Circuit};
    use itertools::Itertools;
    use pasta_curves::arithmetic::FieldExt;
    use pasta_curves::pallas;
    use std::convert::TryInto;
    use std::marker::PhantomData;

    #[test]
    fn test_rho_gate() {
        #[derive(Default)]
        struct MyCircuit<F> {
            in_state: [F; 25],
            _marker: PhantomData<F>,
        }
        impl<F: FieldExt> Circuit<F> for MyCircuit<F> {
            type Config = RhoConfig<F>;
            type FloorPlanner = SimpleFloorPlanner;

            fn without_witnesses(&self) -> Self {
                Self::default()
            }

            fn configure(meta: &mut ConstraintSystem<F>) -> Self::Config {
                let state: [Column<Advice>; 25] = (0..25)
                    .map(|_| meta.advice_column())
                    .collect::<Vec<_>>()
                    .try_into()
                    .unwrap();
                RhoConfig::configure(meta, state)
            }

            fn synthesize(
                &self,
                config: Self::Config,
                mut layouter: impl Layouter<F>,
            ) -> Result<(), Error> {
                let state = layouter.assign_region(
                    || "assign input & output state + constant in same region",
                    |mut region| {
                        let offset = 0;
                        let state: [Lane<F>; 25] = self
                            .in_state
                            .iter()
                            .enumerate()
                            .map(|(idx, value)| {
                                let cell = region
                                    .assign_advice(
                                        || format!("lane {}", idx),
                                        config.state[idx],
                                        offset,
                                        || Ok(*value),
                                    )
                                    .unwrap();
                                Lane {
                                    cell,
                                    value: *value,
                                }
                            })
                            .collect::<Vec<_>>()
                            .try_into()
                            .unwrap();
                        Ok(state)
                    },
                )?;
                let next_state =
                    config.assign_rotation_checks(&mut layouter, state)?;
                layouter.assign_region(
                    || "assign input & output state + constant in same region",
                    |mut region| {
                        let offset = 1;
                        config.assign_region(
                            &mut region,
                            offset,
                            next_state.clone(),
                        )?;
                        Ok(())
                    },
                )?;
                Ok(())
            }
        }

        let input1: State = [
            [1, 0, 0, 0, 0],
            [0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0],
        ];
        let mut in_biguint = StateBigInt::default();
        let mut in_state: [pallas::Base; 25] = [pallas::Base::zero(); 25];

        for (x, y) in (0..5).cartesian_product(0..5) {
            in_biguint[(x, y)] = convert_b2_to_b13(input1[x][y]);
        }
        let s0_arith = KeccakFArith::theta(&in_biguint);
        for (x, y) in (0..5).cartesian_product(0..5) {
            in_state[5 * x + y] =
                biguint_to_f(s0_arith[(x, y)].clone()).unwrap();
        }
        let s1_arith = KeccakFArith::rho(&s0_arith);
        let mut out_state: [pallas::Base; 25] = [pallas::Base::zero(); 25];
        for (x, y) in (0..5).cartesian_product(0..5) {
            out_state[5 * x + y] =
                biguint_to_f(s1_arith[(x, y)].clone()).unwrap();
        }
        let circuit = MyCircuit::<pallas::Base> {
            in_state,
            _marker: PhantomData,
        };
        // Test without public inputs
        let prover =
            MockProver::<pallas::Base>::run(9, &circuit, vec![]).unwrap();

        assert_eq!(prover.verify(), Ok(()));
    }
}
