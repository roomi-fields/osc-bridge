# CR — Bridge N/N pour intégration Kanopi

## Contexte

osc-bridge aujourd'hui = **1 synthé ↔ 1 client**, orienté configuration
(CC, NRPN, SysEx). Kanopi a besoin d'un **N synthés ↔ N clients**
orienté performance (jouer des notes, observer des retours en direct,
savoir si le setup est up). Quatre gaps identifiés par Kanopi, tranchés
avec l'architecte.

**Règle d'or :** zéro régression sur l'existant. Les devices JSONs
déjà livrés doivent continuer à fonctionner sans modification.

## Phase 1 — `midi_out` (bloquant Kanopi)

Sans ça Kanopi ne peut pas jouer une note.

### Schéma JSON ajouté

Section optionnelle au niveau racine du device :

```json
"midi_out": {
  "default_channel": 0,
  "note_offset": 0
}
```

Présence de la section = **active automatiquement** les routes OSC
standards, préfixées par `device.osc_prefix` :

| OSC path                              | MIDI bytes                                   |
|---------------------------------------|----------------------------------------------|
| `/note/on {note} {vel} [{channel}]`   | `0x90 \| ch, note + note_offset, vel`        |
| `/note/off {note} {vel} [{channel}]`  | `0x80 \| ch, note + note_offset, vel`        |
| `/pitchbend {value_u14} [{channel}]`  | `0xE0 \| ch, lsb, msb`                       |
| `/aftertouch {value} [{channel}]`     | `0xD0 \| ch, value` (channel pressure)       |
| `/poly_aftertouch {note} {val} [{c}]` | `0xA0 \| ch, note, value`                    |
| `/cc/{num} {value} [{channel}]`       | `0xB0 \| ch, num, value`                     |
| `/program_change {prog} [{channel}]`  | `0xC0 \| ch, prog`                           |

Le dernier argument `channel` est optionnel. S'il est absent,
`default_channel` s'applique.

### Overrides supportés (architect)

- `default_channel` (u8, 0–15) : canal MIDI par défaut.
- `note_offset` (i8, -64..+63) : décalage appliqué à toutes les notes
  entrantes. Couvre les drum-machines avec un mapping note custom
  sans ouvrir la porte aux templates personnalisés.

Pas d'autre override au départ. Si un cas exotique surgit (MPE,
channel-rotation), on l'ajoutera comme champ dédié, pas comme template
générique.

### Modifications

- `src/device.rs` : nouveau struct `MidiOut { default_channel: u8, note_offset: i8 }`,
  ajouté à `Device` en `Option<MidiOut>`.
- `src/runtime.rs` : dans `handle_message`, si `device.midi_out.is_some()`
  et que `msg.addr` matche l'un des 7 patterns standards → construire
  les bytes MIDI et enqueue. Route prioritaire avant les commands/cc_params.
- `src/main.rs` : inspect affiche la section midi_out si présente.

### Tests

- Fichier `tests/midi_out.rs`.
- Note on canal default (0x90 00 3C 64 pour note 60, vel 100, ch 1).
- Note on canal overridé.
- Note avec note_offset (drums).
- Pitchbend (0xE0 LSB MSB correct).
- CC (`/cc/74 80` → 0xB0 4A 50).
- Rétro-compat : MiniLab 3 continue sans midi_out, rien ne change.

## Phase 2 — `/bridge/status` (debug silencieux → bruyant)

Endpoint OSC interrogeable pour savoir au boot si tout est up. Pas de
reply-side-effects ailleurs, juste une RPC.

### Routes exposées

- `/bridge/status` (aucun arg) → le bridge répond sur *tous* les osc
  clients configurés :
  - `/bridge/status/device <device-name> <state>` — une fois par device
    déclaré.
  - `state` ∈ `"ok"` | `"midi_in_closed"` | `"midi_out_closed"` | `"no_midi"`.

En mode single-device (aujourd'hui), un seul message `/bridge/status/device`.
En mode multi-device (phase 4), N messages.

### Modifications

- `src/runtime.rs` : intercepter le path `/bridge/status` avant les
  autres dispatches. Émettre via le `send_osc` existant.
- Aucun changement de schéma device.

### Tests

- Ping → pong : envoyer `/bridge/status`, vérifier qu'un message
  `/bridge/status/device <name> ok` est renvoyé à l'osc-client.
- Message reçu quand le port MIDI-out n'est pas résolu : state correct.

## Phase 3 — Multi-client broadcast

### CLI

`--osc-client <addr>` devient répétable :

```bash
osc-bridge run --device X --osc-client 127.0.0.1:8888 --osc-client 127.0.0.1:9999
```

### Modifications

- `src/main.rs` : `osc_client: Option<String>` → `osc_clients: Vec<String>`.
- `src/runtime.rs` : `RuntimeOptions.osc_client: Option<SocketAddr>` →
  `osc_clients: Vec<SocketAddr>`. `send_osc` envoie à tous.
- Pas de subscribe/unsubscribe au départ. Tous les clients reçoivent
  tous les events (YAGNI).

### Tests

- Deux sockets UDP locales, envoyer un `/cc/…` simulé, vérifier que
  les deux reçoivent le message correspondant.

## Phase 4 — `orchestrate` multi-device

### CLI

```bash
osc-bridge orchestrate --config bridge.toml
```

### Fichier `bridge.toml`

```toml
[osc]
bind = "127.0.0.1:7777"
clients = ["127.0.0.1:8888"]

[[devices]]
spec = "devices/moog/subsequent-37.json"
midi_out_port = 3
midi_in_port = 2

[[devices]]
spec = "devices/arturia/matrixbrute.json"
osc_prefix = "/matrixbrute-1"     # override : permet deux instances du même synthé
midi_out_port = 5
midi_in_port = 4
```

### Modifications

- Nouveau fichier `src/orchestrator.rs` — un `Orchestrator` qui :
  - charge N `Device`s,
  - overrideles `osc_prefix` quand présent dans le TOML,
  - ouvre N connexions MIDI-out + N MIDI-in,
  - spawn un thread par MIDI-in (chacun taggé avec le device),
  - écoute un **seul** socket OSC,
  - dispatch par préfixe.
- `src/main.rs` : sous-commande `Orchestrate { config: PathBuf }`.
- Dépendance : `toml = "0.8"`.

### Tests

- Config TOML avec deux devices différents (fichiers JSON existants).
- Envoyer `/sub37/...` → vérifier que ça part sur le port MIDI 3.
- Envoyer `/matrixbrute-1/...` → port 5.
- `/bridge/status` → deux messages `/bridge/status/device`.
- `/bridge/status/device matrixbrute-1 ok` utilise le préfixe override,
  pas celui du JSON.

## Dépendances

Ajouter dans `Cargo.toml` :

```toml
toml = "0.8"
```

Uniquement utilisée par l'orchestrator (phase 4). Pas de feature flag,
l'overhead est négligeable.

## Ordre de livraison

1. Phase 1 `midi_out` → débloque Kanopi pour jouer des notes.
2. Phase 2 `/bridge/status` → supprime le debug silencieux.
3. Phase 3 Multi-client → confort, permet visualizer + logger en parallèle.
4. Phase 4 `orchestrate` → scale-out, permet N devices dans un process.

Chaque phase = un commit atomique + tests + bump de version (v0.6.0 → v0.9.0)
ou regroupage si deux phases arrivent vite. À voir au fur et à mesure.

## Critères d'acceptation

- Tous les devices JSON actuels se chargent et fonctionnent sans
  modification (zéro régression).
- `cargo test --release` passe sur chaque phase.
- README + `docs/DEVICE_JSON_SCHEMA.md` + `CHANGELOG.md` à jour à
  chaque phase.
- Un device peut, au choix, activer ou non `midi_out` — l'absence de
  la section ne change rien au comportement actuel.

## Non-goals explicites

- Pas de MPE dédié au premier jet (notes per-channel) — on attend un
  vrai use case.
- Pas de filtrage par client (`/bridge/subscribe /sub37`) — YAGNI.
- Pas de hot-reload de `bridge.toml` — un redémarrage osc-bridge ne
  coûte rien.
- Pas de discovery mDNS / Bonjour — Kanopi connaît les ports à
  l'avance via sa propre config.
