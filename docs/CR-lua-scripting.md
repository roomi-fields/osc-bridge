# CR — Intégration Lua comme échappatoire scriptée

## Contexte

osc-bridge est un bridge MIDI↔OSC **déclaratif** : les devices sont décrits en JSON pur. Cette approche couvre ~95% des cas (CC, NRPN, SysEx templated, reply-matching, encodages u7/u14/ASCII). Pour les 5% restants — checksums exotiques, encodages non standards, transformations conditionnelles complexes — il faut une échappatoire.

Choix retenu : **Lua embarqué via `mlua`**, utilisable uniquement dans des champs explicites des specs JSON. Lua, pas Rhai ni WASM, parce que :
- public cible (sound-designers, users Renoise/REAPER/VCV) connaît déjà Lua
- `mlua` est mature, sûr, bien intégré à l'écosystème Rust
- syntaxe lisible sans être dev

**Règle d'or** : un script Lua est une exception documentée, pas une option par défaut. Le déclaratif reste le chemin principal. Le linter doit warn à chaque usage de `script`.

## Objectifs

1. Ajouter un moteur Lua sandboxé au runtime.
2. Exposer des points d'extension dans le schéma JSON des devices (champs optionnels).
3. Définir une API Lua minimale (helpers SysEx, accès aux args, retour de valeurs).
4. Pas de régression sur les specs existantes (retro-compat totale).

## Dépendances

```toml
[dependencies]
mlua = { version = "0.10", features = ["lua54", "vendored", "send"] }
```

- `vendored` : compile Lua 5.4 statiquement, zéro dépendance système.
- `send` : permet de passer le Lua entre threads si besoin futur.
- Pas de `unsafe` user-facing ; `mlua` encapsule tout.

## Points d'extension dans les specs JSON

Trois niveaux, du plus léger au plus lourd. **Implémenter dans l'ordre.**

### Niveau 1 — `transform` (expression inline)

Champ `transform` optionnel sur un `param` ou un `command.arg`. Exécuté comme une expression Lua, reçoit `value` en entrée, renvoie la valeur transformée.

```json
{
  "name": "cutoff",
  "cc": 74,
  "transform": "return math.floor(value * 127 / 1000)"
}
```

Sens : OSC→MIDI. Pour MIDI→OSC, un champ jumeau `transform_reverse` symétrique.

### Niveau 2 — `script` (block par commande/param)

Champ `script` sur une `command` ou un `reply_pattern`. Reçoit un contexte structuré, renvoie un message transformé ou `nil` pour filtrer.

```json
{
  "command": "weird_checksum_cmd",
  "sysex_template": "F0 00 20 6B 7F 42 {payload} {checksum} F7",
  "script": "ctx.checksum = 0; for _, b in ipairs(ctx.payload) do ctx.checksum = ctx.checksum ~ b end; return ctx"
}
```

### Niveau 3 — `codec` (device entier, optionnel)

Pour protocoles vraiment exotiques : un fichier `.lua` externe référencé dans la spec, qui implémente deux fonctions `encode(osc_msg) -> midi_frame` et `decode(midi_frame) -> osc_msg`. À réserver aux cas où le déclaratif ne tient pas du tout. **Ne pas implémenter au premier jet** — attendre un use case réel.

## Architecture

### Module `src/scripting.rs` (nouveau)

- `struct ScriptEngine` : wrapper autour de `mlua::Lua`, avec pool ou instance par device (choix : **une instance par device** pour isolation, coût mémoire négligeable).
- Injection des helpers standards au démarrage (voir API ci-dessous).
- `fn eval_transform(&self, expr: &str, value: i64) -> Result<i64>`
- `fn run_script(&self, code: &str, ctx: ScriptContext) -> Result<ScriptContext>`
- Sandboxing : désactiver `os`, `io`, `package`, `require`, `dofile`, `loadfile`. Garder `math`, `string`, `table`, `bit32`.

### Modifications

- `src/device.rs` : étendre les structs `Param`, `Command`, `ReplyPattern` avec `transform: Option<String>`, `transform_reverse: Option<String>`, `script: Option<String>`.
- `src/runtime.rs` : câbler l'appel au `ScriptEngine` aux bons endroits dans les pipelines OSC→MIDI et MIDI→OSC.
- `src/frame.rs` : exposer les helpers d'encodage (u7, u14, checksum xor/sum) aussi côté Lua.

### API Lua exposée

Module global `ob` (pour *osc-bridge*) :

```lua
ob.u14_lsb(v)         -- renvoie les 7 bits bas
ob.u14_msb(v)         -- renvoie les 7 bits hauts
ob.u7_clamp(v)        -- clamp [0, 127]
ob.checksum_xor(t)    -- XOR d'une table de bytes
ob.checksum_sum(t)    -- somme modulo 128
ob.log(msg)           -- log debug, jamais en prod
```

Contexte passé à un `script` :

```lua
ctx = {
  args = { ... },      -- args OSC décodés
  payload = { ... },   -- bytes SysEx bruts (MIDI→OSC) ou à remplir (OSC→MIDI)
  checksum = nil,      -- à remplir si besoin
  direction = "osc_to_midi" | "midi_to_osc",
  device = "minilab3",
  command = "set_pad_color",
}
```

## Sécurité et limites

- **Sandbox strict** : pas d'accès fichier/réseau/process.
- **Timeout** par exécution : 10 ms hard cap via `Lua::set_memory_limit` et un hook d'instructions. Un script qui boucle ne doit jamais bloquer le runtime.
- **Memory cap** : 1 MB par instance Lua. Largement au-delà de ce qu'un transform honnête consomme.
- **Pas d'état persistant entre appels** par défaut. Si besoin futur, ajouter un champ `state` dans `ctx` explicitement.
- Les erreurs Lua remontent comme `BridgeError::ScriptError` sans crasher le runtime ; le message est dropped avec log warn.

## Tests

Fichier `tests/scripting.rs` :

1. `transform` simple : scaling linéaire, clamp.
2. `transform` avec `math` : courbe exponentielle pour un volume.
3. `script` avec checksum XOR sur payload variable.
4. Sandbox : vérifier que `os.execute("rm -rf /")` échoue à la compilation.
5. Timeout : un `while true do end` doit échouer en < 20 ms avec `ScriptError::Timeout`.
6. Memory cap : allocation > 1 MB doit échouer proprement.
7. Retro-compat : tous les devices existants doivent passer sans modification.

## Documentation

- `docs/scripting.md` : guide user. Cas d'usage, API `ob.*`, exemples courts. **Commencer par un avertissement** : "N'utilisez ceci que si le déclaratif ne suffit pas. 95% des devices n'en ont pas besoin."
- Mettre à jour le README pour mentionner l'échappatoire en une ligne, pas plus.
- Ajouter un exemple de device minimal avec `transform` dans `devices/examples/`.

## Linter

Ajouter une sous-commande `osc-bridge lint <device.json>` qui :
- valide le schéma
- **warn sur chaque champ `script` ou `transform`** avec le message : "Scripted fallback used — prefer declarative if possible."
- erreur si un champ `codec` pointe vers un fichier absent

## Non-goals explicites

- Pas de REPL Lua interactif.
- Pas de chargement dynamique de modules Lua externes (sauf niveau 3 `codec`, à voir plus tard).
- Pas de bindings MIDI/OSC directs côté Lua : le script transforme des valeurs, il n'envoie pas lui-même.
- Pas de partage d'état entre scripts de devices différents.

## Ordre de livraison suggéré

1. Module `scripting.rs` + sandbox + tests unitaires engine seul.
2. Champ `transform` sur `param` — chemin le plus court, valeur user immédiate.
3. Champ `script` sur `command` et `reply_pattern`.
4. Linter.
5. Doc user + exemple.
6. (Plus tard, si demande réelle) niveau 3 `codec`.

## Critères d'acceptation

- `cargo test` passe, y compris tests existants.
- Tous les devices actuels (`devices/**/*.json`) se chargent et fonctionnent sans modification.
- Un device exemple avec `transform` et `script` est livré et testé.
- Un script malveillant (`os.execute`, boucle infinie, alloc géante) est bloqué proprement.
- Overhead runtime d'un device *sans* script : zéro mesurable (le `ScriptEngine` est lazy-init).
