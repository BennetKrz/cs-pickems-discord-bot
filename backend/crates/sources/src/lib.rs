//! Data-source adapters.
//!
//! Adapters only fetch and convert external data into source-neutral DTOs;
//! they never touch the database. The `SourceAdapter` trait arrives in T03,
//! the PandaScore adapter in T04 and the manual/fixture sources in T05.
