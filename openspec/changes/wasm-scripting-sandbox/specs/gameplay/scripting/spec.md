# Capability: Sandboxed WASM Gameplay Scripting

## Purpose
Defines requirements for the isolated, fuel-metered WebAssembly scripting engine, authoritative host API bindings, and atomic hot-reloading pipeline.

## ADDED Requirements

### Requirement: Sandboxed WASM Module Execution
The engine SHALL execute WebAssembly modules in a strictly isolated virtual environment without direct access to the host filesystem, sockets, hardware clocks, or raw OS memory.

#### Scenario: Script executes valid arithmetic or game calculation
- **GIVEN** a compiled WASM module with an exported calculation function
- **WHEN** the host invokes the function with input parameters
- **THEN** the execution SHALL succeed, return the expected result, and consume fuel proportional to instructions executed.

#### Scenario: Script attempts prohibited system operations
- **GIVEN** a WASM module attempting to import unprovided OS/WASI primitives (e.g., file descriptors or network sockets)
- **WHEN** module instantiation or execution is attempted
- **THEN** the engine SHALL reject module instantiation or link resolution with a typed error without impacting host stability.

---

### Requirement: Deterministic Fuel / Gas Metering
The engine SHALL enforce strict fuel limits on guest WASM execution to ensure no script can exceed tick CPU budgets or execute unbounded loops.

#### Scenario: Runaway loop terminated by fuel exhaustion
- **GIVEN** a guest script containing an infinite loop (`loop {}`) or computationally excessive calculation
- **WHEN** the script function is invoked with a fuel limit of 50,000 units
- **THEN** the execution SHALL terminate deterministically with `ScriptError::OutOfFuel`, the host call SHALL return without panicking, and the simulation tick loop SHALL remain unharmed.
- **AND** if invoked within a synchronous hook (e.g. `BeforeInteract`), the hook context SHALL fail closed by calling `ctx.cancel("script_error: out of fuel")`.

#### Scenario: Script trap during hook execution (Fail-Closed)
- **GIVEN** a guest script that triggers a WebAssembly trap (e.g. divide by zero or unreachable)
- **WHEN** the hook handler receives the trap error
- **THEN** the hook SHALL fail closed and cancel the action with a descriptive cancellation reason, preventing unintended exploits.

#### Scenario: Normal script consumes fuel within budget
- **GIVEN** a guest script that executes within the allotted fuel budget
- **WHEN** the script completes execution
- **THEN** the remaining fuel SHALL be accurately calculated and reported back to the host runtime.

---

### Requirement: Authoritative Host API Bindings
The engine SHALL provide typed, safe host bindings enabling scripts to inspect entity positions, read and mutate entity attributes, calculate spatial distances, and dispatch events.

#### Scenario: Script reads and updates entity position
- **GIVEN** an active entity in the simulation world with position `(10.0, 20.0, 30.0)`
- **WHEN** a WASM script invokes `entity_get_position` and subsequently `entity_set_position(15.0, 25.0, 35.0)`
- **THEN** the host entity position SHALL be validated, updated, and reflected in the entity store.

#### Scenario: Script accesses entity attribute
- **GIVEN** an entity with an attached attribute or health component
- **WHEN** a WASM script queries or modifies the attribute via the Host API
- **THEN** the host SHALL perform bounds checking using 64-bit integer arithmetic on guest memory pointers and safely apply the mutation or return the value.

#### Scenario: Malicious or invalid guest memory pointer
- **GIVEN** a WASM script that passes an out-of-bounds pointer or integer-wrapping length (e.g., `ptr = 0xFFFFFFFF`, `len = 2`)
- **WHEN** the host API attempts to read or write guest memory
- **THEN** the host SHALL reject the operation safely via 64-bit checked bounds evaluation `(ptr as u64) + (len as u64) <= memory_size`, returning an error code and never panicking.

---

### Requirement: Atomic Module Hot-Reloading
The engine SHALL support live hot-reloading of compiled script modules at tick boundaries without dropping active entity states or interrupting connected players.

#### Scenario: Hot-reloading an updated ability script
- **GIVEN** an active simulation running an existing version of an ability script
- **WHEN** an updated script payload is compiled and hot-reloaded for that module ID
- **THEN** the module registry SHALL atomically swap the compiled module, all subsequent invocations SHALL execute the new script version, and active entities SHALL retain their states.

#### Scenario: Malformed script hot-reload rejection
- **GIVEN** an active simulation running a valid script
- **WHEN** a hot-reload request provides invalid WASM bytecode or unresolvable imports
- **THEN** the compilation/linking SHALL fail with an error, the existing valid module SHALL remain active without interruption, and the error SHALL be logged.
