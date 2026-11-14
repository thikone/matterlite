# mm_model fixtures

JSON samples for the serde tests. They are written by hand from the Go structs in the server's
`server/public/model` (field names, `null` for nil slices, maps and pointers, `omitempty`
fields left out) and include fields from newer servers that the model must ignore.

Replace them with responses captured from a real server when the integration setup exists
(`docs/M1.md`); keep the unknown-field and `null` cases.
