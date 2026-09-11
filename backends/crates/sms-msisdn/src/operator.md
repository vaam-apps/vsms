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

# This type stays country-blind on purpose

Now that [`Msisdn`][crate::Msisdn] parses every country, an obvious next
thought is to give this type a country dimension too. It does not need one:
it already takes untyped `(prefix, operator)` pairs and does
longest-prefix-match against a national number, with no opinion about which
country produced either. The country belongs on the **rows**, in the
database, where a national `67` for Cameroon and a national `67` for France
become two different rows rather than one ambiguous one — that is a schema
change, not a change here.

And note what does *not* get better with wider reach. The `phonenumber`
crate ships no carrier mapper, and could not usefully: mobile number
portability means a prefix stops identifying a network the moment a
subscriber ports, which in the USA, the UK and India is routine. So for most
foreign traffic the honest answer is that the operator is unknown, and every
consumer already has to handle that — `None` here is a normal outcome, not
an edge case.
