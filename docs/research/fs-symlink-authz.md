---
title: Autorização de symlink no node:fs, modelo de ameaça e testes de abuso
ticket: VELYR-179
status: research
date: 2026-10-16
related: [VELYR-77]
---

# Autorização de symlink no node:fs (VELYR-179)

## 1. Resumo

- O fallback do VELYR-77 (entrada `Entry` que é symlink autorizada pelo diretório pai) não abre furo: toda operação `Entry` (lstat, readlink, unlink, rmdir, rename, rm, symlink, origem do cp) age sobre a própria entrada e nunca segue o componente final. Trocar o link por arquivo, diretório ou outro link entre a checagem e o job não leva a operação para fora do diretório pai autorizado.
- Achado crítico, corrigido: as ops assíncronas eram autorizadas quando a chamada era feita, mas o job só roda depois do turno de JS atual (o `tokio::spawn` vai para o runtime `current_thread` da isolate). Nesse intervalo, o próprio script podia trocar um diretório pai, ou plantar um link no nome final, e o job seguia o link. Isso dava escrita, leitura, remoção, `mkdir`, `mkdtemp`, `chmod`, `copyFile` e `utimes` fora do grant, de forma **determinística** (sem corrida). Era um escape de sandbox reproduzível em uma linha.
- Correção (mitigação): todo job assíncrono re-resolve, logo antes do I/O, cada caminho que o `prepare` autorizou (`verify_unchanged`). O caminho precisa continuar resolvendo para ele mesmo. O ataque determinístico morreu. Sobra uma janela de microssegundos entre a re-checagem e a syscall. Fechar essa janela exige resolução relativa a fd, o que é decisão humana (seção 5).
- Achado aberto, fora do escopo de symlink: operações de árvore autorizam só a raiz. Um `deny` aninhado dentro do grant é lido por `cp -r`, movido por `rename` e apagado por `rm -r`. Há um teste `#[ignore]` que reproduz o caso.

## 2. Modelo de ameaça

- **Ativo:** tudo fora do grant (`--allow-read`/`--allow-write`) e tudo coberto por um `deny` dentro dele.
- **Atacante:** o próprio código JS/TS rodando no sandbox. Ele controla o conteúdo do grant de escrita (pode criar links com alvo arbitrário, renomear diretórios) e controla a ordem das chamadas: pode emitir uma op assíncrona e mexer na árvore no mesmo turno síncrono, antes de o job rodar.
- **Fronteira:** `authorize_path` (`crates/io-fs/src/sync_ops.rs`) resolve o caminho para a forma canônica, autoriza essa forma e devolve o caminho que o job deve usar. Os jobs (`crates/compat-node/src/fs_ops.rs`, `fs_jobs.rs`, `fs_copy.rs`) usam `std::fs`, que resolve o caminho de novo no kernel. É essa segunda resolução que o atacante tenta desviar.
- **Fora do modelo:** um processo externo concorrente mexendo na árvore (mesma classe da janela residual), e a semântica do crate de permissões (ele resolve symlinks ao checar).

## 3. Ataques e veredito

| # | Ataque | Teste (`crates/compat-node/tests/fs_security.rs`) | Veredito |
|---|--------|------------------------------------------------------|----------|
| 1 | TOCTOU: op assíncrona autorizada, diretório pai trocado por link para fora antes do job (writeFile, readFile, rm -r, mkdtemp, mkdir, copyFile, utimes, unlink, chmod, readdir, stat, rename) | `toctou_parent_swap_cannot_redirect_async_jobs` | **corrigido** (RED antes: `["unlink","chmod","writeFile","rm","mkdtemp","mkdir","copyFile"]` escaparam) |
| 2 | TOCTOU: writeFile/appendFile assíncrono para nome novo, link plantado nesse nome antes do job | `toctou_link_planted_at_a_new_name_is_not_followed` | **corrigido** (RED antes: o write seguiu o link) |
| 3 | TOCTOU no fallback do VELYR-77: rm/unlink/rename de um link, link trocado por outro link ou por diretório antes do job | `toctou_swapping_the_fallback_link_stays_inside_the_grant` | bloqueado (regressão) |
| 4 | rename de link para fora do grant, rename vindo de fora, rename sobre um link, rename para dentro de um diretório que é link para fora | `rename_cannot_move_a_link_out_of_the_grant` | bloqueado (regressão) |
| 5 | link criado apontando para fora e usado por readFile, stat, access, realpath, readdir, opendir, open (r/w), copyFile (origem e destino), writeFile, appendFile, chmod, utimes, mkdir, mkdtemp, rm, unlink, cp, symlink através do link, createReadStream, createWriteStream; `mkdir -p` e writeFile através de link pendente | `a_planted_link_cannot_be_used_by_other_apis` | bloqueado (regressão) |
| 6 | cp e rm recursivo em árvore com links para fora: cp copia o link como link; destino com link para diretório não é mesclado (EEXIST); `force` substitui o link e não escreve no alvo; rm remove o link e não o alvo | `cp_and_recursive_rm_never_follow_links_in_a_tree` | bloqueado (regressão) |
| 7 | mkdtemp com prefixo que sobe (`..`), que passa por link, que termina em `/` dentro de link, ou cujo `<prefix>XXXXXX` já existe como link para fora | `mkdtemp_prefix_cannot_leave_the_grant` | bloqueado (regressão) |
| 8 | `deny` dentro do grant: link para o caminho negado não é seguido; o fallback só age sobre o link | `links_into_a_denied_subtree_cannot_be_followed` | bloqueado (regressão) |
| 9 | `deny` aninhado sob a raiz de uma op de árvore (cp -r, rename, rm -r) | `tree_operations_respect_a_nested_deny` (`#[ignore]`) | **aberto** (cp confirmado por teste; rename e rm pela leitura do código, já que autorizam só a raiz) |

Notas por API:

- **readFile / createReadStream / open:** `PathMode::Follow` (ou `Create` com flags de escrita) resolve o link inteiro e autoriza o alvo. Não existe fallback fora de `Entry`. `open` faz o `open(2)` dentro do `prepare`, então não tem janela; por isso ele não registra guarda (registrar vazaria o fd se a re-checagem falhasse depois do open).
- **cp:** a origem é `Entry`, mas `copy_tree` usa `symlink_metadata` e copia links como links, então autorizar a origem pelo pai não dá leitura do alvo. O destino é `Create`, que resolve um link no topo e nega.
- **mkdtemp:** autorizado como `<prefix>XXXXXX` em modo `Create`. O pai canônico precisa estar no grant. Link pendente no placeholder é recusado e link existente é resolvido e checado.

## 4. A correção

- `crates/io-fs/src/sync_ops.rs`: `verify_unchanged(path, mode, syscall, raw)` roda `resolve(path, mode)` de novo e exige o mesmo caminho. Se o resultado divergir (link no meio, link plantado no nome final, link pendente), devolve `ERR_ACCESS_DENIED` com cause `VELYR_PERMISSION_DENIED`. `NotFound` passa direto e a própria op reporta `ENOENT`, para não mudar o código de erro de corridas benignas (ex.: `rm({ force: true })` em algo apagado no meio). Criar através de um pai pendente falha no kernel: `mkdir`/`open(O_CREAT)` não criam o pai, e `create_dir_all` recusa um link pendente no lugar de um diretório (coberto pelo teste 5).
- `crates/compat-node/src/fs_ops.rs`: `prepare` registra cada caminho autorizado (`authorize` → `Guard`). `op_async_call` embrulha o job em `guarded`, que chama `verify_unchanged` em todas as guardas antes do trabalho. `op_sync` não re-checa, porque entre a checagem e o I/O não roda JS.

## 5. Risco residual e decisão humana

1. **Janela residual do TOCTOU (alto, exige corrida):** entre `verify_unchanged` e a syscall ainda existe uma janela curta. Um script que dispara muitas ops assíncronas de `rename`/`symlink` no blocking pool, em paralelo com o alvo, pode ganhar a corrida de vez em quando. O mesmo vale para o walk recursivo de `cp`/`rm`, que resolve cada filho por caminho. Fechar de verdade exige resolução relativa a fd: `openat` componente a componente com `O_NOFOLLOW`, `openat2(RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS)` no Linux, `O_NOFOLLOW_ANY` no macOS, ou o crate `cap-std`. Todos pedem dependência nova ou `unsafe` (o compat-node proíbe `unsafe`, e o `Cargo.toml` não estava no escopo deste ticket). Decisão: adotar `cap-std` (ou equivalente) no io-fs e passar a fazer os jobs sobre dirfd.
2. **Deny aninhado em op de árvore (médio/alto quando há `deny`):** ataque 9. As opções são: (a) no `prepare`, percorrer a árvore e autorizar cada entrada (custo O(árvore) no thread da isolate); (b) pedir ao crate de permissões uma API "existe deny abaixo de X?" e negar a op de árvore nesse caso (mexe no crate de permissões, decisão humana); (c) documentar como limitação, como o Deno faz.
3. **Deny sobre um symlink (baixo):** o crate de permissões resolve o padrão ao construir o set, então `deny` em `DIR/link` vira deny no alvo do link. Unlink/rename do próprio link caem no fallback pelo pai e são permitidos. Se a intenção for proteger a entrada link, a semântica precisa mudar no crate de permissões.
4. **Funcional, não segurança:** `mkdtemp` com prefixo que já termina em `XXXXXX` (`trim_end_matches` corta todas as repetições) cria um diretório com nome diferente do que é devolvido. Fora do escopo.

## 6. Como reproduzir

```
source ~/.cargo/env && cd /Users/lucasbrito/dev/wdt/velyr-core
cargo test -p velyr-compat-node --test fs_security
cargo test -p velyr-compat-node --test fs_security tree_operations -- --ignored   # achado aberto
cargo test -p velyr-compat-node --test fs
```
