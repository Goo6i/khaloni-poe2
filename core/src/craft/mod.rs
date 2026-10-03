//! The craft planner's model: the mod pool an item can roll from, the
//! rules each currency follows, and the costing of strategies. Pure: no
//! file, network or clock access.

pub mod data;
pub mod model;
pub mod observed;
pub mod plan;
pub mod pool;
pub mod rules;
pub mod sim;
pub mod state;
pub mod strategy;
pub mod types;
