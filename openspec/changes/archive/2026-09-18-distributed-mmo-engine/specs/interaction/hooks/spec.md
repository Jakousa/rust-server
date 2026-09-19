## Purpose

Provides a synchronous event hook pipeline for decoupled modules to intercept, mutate, and veto player and entity interactions.

## ADDED Requirements

### Requirement: Synchronous Event Hook Pipeline
The core engine SHALL dispatch gameplay lifecycle events through a synchronous pipeline passing a mutable EventContext.

#### Scenario: Intercepting an interaction event
- **WHEN** an interaction event (e.g. trade request, world item pickup) occurs
- **THEN** registered module handlers SHALL execute in order and have access to the EventContext

### Requirement: Action Veto and Mutation
Module handlers SHALL have the ability to inspect entity states, mutate modular component data, or veto and cancel the pending action.

#### Scenario: Vetoing an interaction
- **WHEN** a module handler marks the event context as canceled (e.g., inventory full)
- **THEN** the core engine SHALL abort the action and return an error status to the invoking entity
