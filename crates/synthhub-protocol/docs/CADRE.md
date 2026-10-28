# synthhub-protocol — frame

## Role

- **R1** `synthhub-protocol` holds the types exchanged between the SyntHub service and its clients: the versioned service API, the relay frames between the service and a door, and an installation's identity.

## Receives

- **R2** Nothing at run time: it is a set of types with their serialisation.

## Returns

- **R3** The request and response types of each API version, the relay frames, the identity types, and their JSON form.

## Knows

- **R4** The contract and its versions.

## Does not know

- **R5** How either side implements a request, the database, sessions, ports.

## Refuses

- **R6** A frame or response that does not match its version's type: a typed fault at deserialisation.

## Invariants

- **R7** A published API version keeps its types: a change is a new version, and the old one stays defined while installations use it.
- **R8** The service, osc-bridge and the site use these same types; neither side writes the contract by hand.

## Cost

- **R9** Serialising or reading one frame stays under 50 µs.
