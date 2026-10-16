---
title: Proteção contra ReDoS no RegExp da engine nativa e sticky O(1) por tentativa
ticket: VELYR-158
status: research (caminho (a) implementado)
date: 2026-10-16
related: [VELYR-112, ADR-011]
---

# Proteção contra ReDoS e sticky O(1) por tentativa (VELYR-158)

## 1. Resumo

- Recomendação: caminho (a), um fork vendorizado do `regress` 0.12.0 em `crates/engine-vm/vendor/regress`, com um hook de interrupção no backtracker e uma busca ancorada (sticky). Em paralelo, abrir um PR upstream com a mesma API para, no futuro, voltar à dependência do crates.io.
- O patch cabe em cerca de 90 linhas, em 2 arquivos do regress (`src/classicalbacktrack.rs` e `src/api.rs`), mais 1 linha removida do `Cargo.toml`. Licença MIT OR Apache-2.0, a mesma do workspace. Os 544 testes upstream do regress passam no fork (1 ignorado, como upstream).
- Resultado: `/(a+)+$/.test('a'.repeat(40)+'b')` agora para em menos de 100 ms depois de `terminate()`, e a execução sai como `Abrupt::Terminated` (antes ela rodava praticamente para sempre). O budget de CPU usa a mesma flag, então passa a valer para regex também.
- O teste `split_with_a_sticky_splitter_tries_each_position_once` caiu de 12,5-14,6 s para cerca de 0,6-1,2 s (a suíte inteira, em debug). Ele não era regressão da edição parcial. Havia duas causas O(n²) somadas, e as duas foram corrigidas (seção 5).
- (b), um motor alternativo, não compensa hoje: nenhum candidato em Rust puro é compatível com ECMAScript e tem budget ao mesmo tempo. (c), a heurística, fica como defesa opcional; sozinha ela não basta.

## 2. Problema

O `regress` 0.12 não tem limite de passos nem callback de interrupção. `RegExpBuiltinExec` chamava `check_termination()` antes do match, e só. Uma vez dentro do backtracker, um padrão catastrófico como `(a+)+$` roda O(2^n) sem voltar ao interpretador, então ignora `terminate()` e o budget de CPU do runtime. Como a engine nativa é usada como sandbox, isso é um DoS trivial: um script de uma linha trava a thread.

Como o terminate funciona (leitura de `crates/engine-vm/src/interpreter/mod.rs`): `Interpreter` guarda `terminated: Arc<AtomicBool>`, exposto por `termination_flag()`. O `Isolate` do engine-native e o budget de CPU levantam essa flag de outra thread. O interpretador a lê em `check_termination()` nos back-edges e nas chamadas e devolve `Abrupt::Terminated`. O hook do regex lê essa mesma flag. Como `builtins::regexp` é descendente do módulo `interpreter`, ele lê o campo privado `self.terminated` diretamente, sem mudar a API.

## 3. Caminhos avaliados

### (a) Patch no backtracker do regress (escolhido)

O backtracker clássico (`MatchAttempter`) tem um único ponto por onde passa todo retrocesso, `try_backtrack`. Um padrão catastrófico é, por definição, uma explosão de retrocessos, então contar ali cobre o caso. O patch tem quatro partes:

1. `MatchAttempter` ganha `interrupt: Option<&AtomicBool>`, um contador e um estado `interrupted`. A cada 1024 retrocessos (`INTERRUPT_POLL_STEPS`), ele faz um `load(Relaxed)` da flag. Com a flag levantada, `try_backtrack` trunca a pilha até o `Exhausted` e falha. O estado `interrupted` é pegajoso, então lookarounds (que rodam `try_at_pos` recursivo com outra pilha) e o match externo também desistem no retrocesso seguinte.
2. A busca para frente (`next_match_with_prefix_search`) para de avançar posições quando `interrupted` está ativo. Isso cobre o O(n²) sem retrocesso, como um padrão que falha em cada posição de um texto grande.
3. `BacktrackExecutor` ganha `sticky`: tenta só a posição inicial e não avança.
4. Uma API pública nova: `Regex::exec_interruptible(text, start, utf16, sticky, interrupt) -> Option<Match>`. `None` quer dizer "sem match" ou "interrompido", e o chamador distingue os dois relendo a flag.

Custo em tempo de execução: um incremento e um teste de módulo por retrocesso, e um `load` atômico relaxado a cada 1024. Sem interrupção, o comportamento é idêntico ao upstream.

Do lado da engine (`regexp/syntax.rs`), `Matcher::find` recebe a flag e chama `exec_interruptible`. `RegExpBuiltinExec` passa `&self.terminated` e chama `check_termination()` depois do match, de modo que um match interrompido nunca vira `false`/`null` para o script.

Manutenção: o fork fica preso à 0.12.0. Para subir de versão, é preciso reaplicar cerca de 90 linhas em dois arquivos, que mudam pouco upstream. O cabeçalho do `Cargo.toml` vendorizado e o `THIRD_PARTY_NOTICES.md` registram o commit de origem e o que mudou. Um PR upstream com essa API (ou com um `step_limit`) eliminaria o fork; o autor já aceitou opções de execução antes, mas isso não foi verificado com ele.

### (b) Motor alternativo compatível com ECMAScript e com budget

| Candidato | ECMAScript | Budget/interrupção | Observação |
|---|---|---|---|
| `fancy-regex` | Não (sintaxe Perl/Rust, sem UTF-16) | `backtrack_limit` | Semântica diferente (lookbehind, `\b` Unicode, flags); quebraria a conformidade. |
| `regex` (Rust) | Não (sem backreferences e lookaround) | Linear, não precisa | Não implementa o que o JS exige. |
| libregexp do QuickJS | Sim, completa | `lre_check_timeout` (callback) | C via FFI: `unsafe`, build `cc`, mais uma superfície C no sandbox. Licença MIT. |
| PCRE2 / Oniguruma | Parcial (modos de compatibilidade) | `match_limit` / retry limit | C via FFI; divergências de semântica JS; BSD. |
| irregexp (V8) | Sim | Sim | C++ acoplado ao V8; inviável fora dele. |
| `backend-pikevm` do próprio regress | Sim, no subconjunto suportado | Linear | Só existe para entrada UTF-8 (`&str`), não para UTF-16/UCS-2, que a engine usa. |

Conclusão: o único candidato que é ao mesmo tempo compatível e interrompível é a libregexp do QuickJS, que traz C e `unsafe` (o `engine-vm` hoje é `forbid(unsafe_code)`). O custo de trocar de motor é maior que o de manter um patch de cerca de 90 linhas.

### (c) Heurística que rejeita padrões perigosos

Por exemplo, detectar quantificador aninhado sobre um subpadrão que pode casar a mesma coisa de mais de uma forma (`(a+)+`, `(a|a)*`, `(\w|\d)+`). Pode servir como defesa em profundidade, mas não basta sozinha:

- tem falsos negativos: há padrões exponenciais sem aninhamento óbvio (alternâncias sobrepostas, backreferences, lookarounds) e padrões polinomiais (`a*a*a*b` e similares) que nenhuma análise sintática barata pega;
- tem falsos positivos: rejeitar padrões válidos quebra código real e a conformidade com test262;
- não dá garantia nenhuma: o sandbox precisa de um limite duro, e só o hook de (a) fornece isso.

Não foi implementada. Se for desejada, o melhor lugar é um lint ou aviso no `compile` de `regexp/syntax.rs`, nunca uma rejeição.

## 4. Implementação (feita)

| Arquivo | Mudança |
|---|---|
| `crates/engine-vm/vendor/regress/**` | Cópia do `regress-0.12.0` do registry local, com o patch descrito em (a). |
| `crates/engine-vm/Cargo.toml` | `regress = { path = "vendor/regress", version = "=0.12.0", ... }`. |
| `crates/engine-vm/src/interpreter/builtins/regexp/syntax.rs` | `Matcher` com `sticky` e `find(..., interrupt)`; sai o wrap `(?:p)\|()` da edição parcial. `RegExpCache` ganha um cache de um slot com as code units do último texto. |
| `crates/engine-vm/src/interpreter/builtins/regexp.rs` | Em `builtin_exec`: lê `self.terminated`, usa `utf16_of(text)` e chama `check_termination()` depois do match. Esse arquivo fica fora do glob `regexp/**` (ver seção 7). |
| `crates/engine-native/tests/regexp.rs` | Novo teste `terminate_stops_a_catastrophic_backtracking_regexp`. |
| `THIRD_PARTY_NOTICES.md` | Entrada do regress. |

O teste de terminate roda o interpretador numa thread própria (ele não é `Send`), levanta a flag 20 ms depois e espera o resultado com `recv_timeout(100 ms)`. Em RED, ele falhou limpo em 100 ms, sem travar a suíte.

## 5. Sticky e o teste de split de 12,5 s

Diagnóstico por amostragem (`sample` no binário de teste): quase todo o tempo estava em `JsStr::to_utf16`. `builtin_exec` copiava a string inteira (Latin1 para u16) a cada exec, e o split faz um exec por posição: 50 000 cópias de 50 000 unidades. O regex em si quase não aparecia, porque o wrap `(?:p)|()` da edição parcial já ancorava o match.

Contraprova: com a cópia corrigida e o sticky desligado (busca para frente), o mesmo teste levou 102 s. Com o feature `utf16`, o regress não usa prefix search (memchr), então a busca para frente é O(n) por exec.

Conclusão: não é regressão da edição parcial. A edição ancorava corretamente, mas era insuficiente; a cópia O(n) por exec é anterior e independente dela. Sem histórico git (o repositório não tem commits), não dá para provar quando essa cópia entrou. As duas correções são necessárias:

- busca ancorada nativa no fork (`sticky`), que substitui o wrap `(?:p)|()`. O wrap compilava o padrão duas vezes e precisava de um fallback não ancorado (portanto O(n²)) para padrões não-Unicode com `\1`..`\9`, por causa do octal do Annex B. A versão nativa não tem essas exceções;
- um cache de um slot com as code units do texto em `RegExpCache`. O acerto é um `Rc::ptr_eq` quando é a mesma string, que é o caso do split, do replace global e do matchAll.

O cache retém uma cópia do último texto por interpretador, fora da contabilidade do heap do GC. Ficou marcado como `shortcut` no código.

## 6. Verificação

| Comando | Resultado |
|---|---|
| `cargo test -p velyr-engine-native --test regexp` | 11 passed (RED antes: 2 falhas, split 14,6 s e terminate expirado) |
| `cargo test -p velyr-engine-vm` | 50 + 15 + 53 passed |
| `cargo test -p regress` (fork) | todos os testes upstream passam |
| `cargo clippy -p velyr-engine-vm -p velyr-engine-native --all-targets -- -D warnings` | 1 erro preexistente em `builtins/date/parse.rs:269` (`manual_contains`), fora do escopo; com esse lint permitido, limpo, incluindo o fork |
| `bash scripts/ci/check-unsafe.sh` | ok |
| `bash scripts/ci/check-deps.sh` | ok |
| `cargo deny check licenses` | 3 rejeições, igual ao baseline (dragonbox_ecma, webpki-root-certs, webpki-roots) |

## 7. Pontos para decisão humana

1. `builtins/regexp.rs` fica fora do glob permitido `regexp/**` (é o arquivo-raiz do módulo, irmão do diretório). A leitura da flag estava autorizada; a troca de `text.to_utf16()` por `utf16_of(text)` nesse mesmo trecho foi além disso, porque era a causa do teste lento. São 3 linhas em `builtin_exec`. Revisar se o escopo deve incluir esse arquivo.
2. `unsafe` no fork: o código upstream tem cerca de 60 blocos `unsafe` (indexing, position, util, bytesearch...). O patch não adiciona nenhum. Ele já existia como dependência do registry; a diferença é que agora mora no repositório. `check-unsafe.sh` só olha `crates/*/Cargo.toml`, então não vê o vendor. O `unsafe-allowlist.txt` não foi editado. Opção barata: ligar o feature upstream `prohibit-unsafe` (troca os `unsafe` por código checado, com algum custo de desempenho não medido).
3. O vendor virou membro implícito do workspace (path dependency dentro da raiz), então o clippy com `-D warnings` passa a lintar o código do regress. Hoje está limpo; um toolchain novo pode trazer lints no código upstream. A alternativa é um `exclude` no `[workspace]` da raiz.
4. A cópia trouxe os arquivos de registry `.cargo-ok`, `.cargo_vcs_info.json`, `Cargo.lock`, `.github/` e `.gitignore` do pacote. São inofensivos, mas não foram apagados porque a regra proíbe deletar.
5. PR upstream: vale propor `exec_interruptible` (ou um `step_limit`) ao regress para eliminar o fork.
