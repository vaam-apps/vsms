The Cameroon numbering plan.

Closed 9-digit plan since November 2014, when every subscriber number gained
a leading digit: `6` for mobile, `2` for fixed. Eight-digit numbers still sit
in customer databases everywhere and are the single most common bad input.

Ranges, from the ITU national numbering plan and `libphonenumber`:

```text
mobile      (?:24[23]|6(?:[25-9]\d|4[0-2]))\d{6}
fixed line  2(?:22|33)\d{6}
general     [26]\d{8}|88\d{6,7}
```

Note what is *absent* from the mobile range: `63x` and `643`–`649` are not
assigned. `640`–`642` are.
