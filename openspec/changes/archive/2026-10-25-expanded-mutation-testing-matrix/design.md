# Technical Design: Expand Nightly Mutation Testing Matrix to Core, Node, and Gameplay

## 1. Nightly Mutation Workflow Matrix (`.github/workflows/mutation-tests.yml`)

Update `.github/workflows/mutation-tests.yml` so `strategy.matrix.include` defines:
```yaml
    strategy:
      fail-fast: false
      matrix:
        include:
          - name: mmo-schema
            package: mmo-schema
            file_arg: ""
            test_filter: "-- --lib"
          - name: mmo-storage
            package: mmo-storage
            file_arg: ""
            test_filter: "-- --lib"
          - name: mmo-core-collision
            package: mmo-core
            file_arg: "--file crates/core/src/collision.rs"
            test_filter: "-- --lib collision"
          - name: mmo-node-kinematics
            package: mmo-node
            file_arg: "--file crates/node/src/kinematics.rs"
            test_filter: "-- --lib kinematics"
          - name: mmo-gameplay-trade
            package: mmo-gameplay
            file_arg: "--file crates/gameplay/src/trade.rs"
            test_filter: "-- --lib trade"
```
And execute:
```bash
cargo mutants --package "${{ matrix.package }}" ${{ matrix.file_arg }} --jobs 2 ${{ matrix.test_filter }}
```

---

## 2. Boundary & Mutation-Killing Tests

1. **`crates/core/src/collision.rs`**:
   - Add boundary tests for `sweep_test` exact `t_hit` contact ratios (`0.0`, `0.5`, `1.0`), `raycast` layer mask filtering, and `find_path` step-height (`max_step_height` exact equality) and string-pulling line-of-sight checks.
2. **`crates/node/src/kinematics.rs`**:
   - Add boundary tests for `validate_movement` (`dt` clamping, `max_speed` exact boundary `distance == max_allowed_dist`, sequence wrap/replay rejection, `Newtonian6DoF` velocity damping, and `ImpulseAllowance` expiration).
3. **`crates/gameplay/src/trade.rs`**:
   - Add boundary tests for `TradeManager` (`initiate_trade`, `add_item`, `set_gold`, `lock_offer`, `confirm_trade`, `execute_atomic_swap` exact weight capacity limit `current_weight + added == max_weight`, exact gold balance equality, and self-trade/duplicate item rejection).
