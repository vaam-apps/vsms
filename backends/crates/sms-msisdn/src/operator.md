Operator inference, as a lookup against data the caller supplies.

Deliberately without a built-in table. Best current evidence is MTN = `67x`,
`650`–`654` and Orange = `69x`, `655`–`659`, but `68x` is genuinely
contested between sources and Camtel = `62x` is unverified. Number
portability has been legally live since September 2017 and is commercially
near-dead, with sources contradicting each other on whether it works at all.

So prefix inference is right almost always and must never be load-bearing:
it is a routing *hint*, the table lives in the database where operations can
correct it without a deploy, and the delivering network reported on a DLR
overrides it. Compiling a table in here would make all three impossible.
