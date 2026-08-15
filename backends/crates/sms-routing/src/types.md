Input and output types for [`crate::select_route`]. Every type here is
plain data — `Clone`/`Debug`/`PartialEq` throughout, no methods beyond
trivial constructors — because the whole point of this crate is that
its output is inspectable by a caller that never runs the algorithm
itself (#54's admin simulator).
