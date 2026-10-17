# Acesso a arquivo relativo a handle de diretório (R12, RT-16)

Spike de pesquisa. Nenhum código do repo foi alterado; o protótipo vive no scratchpad da sessão e não é commitado
(trechos relevantes estão abaixo). Avalia se vale reabrir o VELYR-181
([fs-symlink-authz.md](fs-symlink-authz.md) seção 5, [SECURITY.md](../SECURITY.md) "Filesystem: known limitations"),
que aceitou a janela TOCTOU residual sem `cap-std`/`openat2`. Os critérios de GO/NO-GO vêm do RT-16 em
[2026-10-17-runtime-memory-plan-2.md](../plans/2026-10-17-runtime-memory-plan-2.md).

## 1. Resumo

| Critério do plano | Resultado | Veredito |
|---|---|---|
| Race test: zero escapes | 0 escapes em 3 x 100k tentativas para cada variante handle-relative (macOS). Linux `openat2`: escrito, NÃO executado. | PASS (macOS) |
| Latência de open <= +10% | `openat` por componente: +145% a profundidade 5, +270% a 10. `O_NOFOLLOW_ANY` e `O_RESOLVE_BENEATH` (1 syscall): -55% a -61% vs o caminho de hoje. | FAIL (por componente), PASS (1 syscall) |
| Decisão nova do Lucas em `docs/SECURITY.md` | Pendente; SECURITY.md não foi editado (decisão do Lucas). | PENDENTE |
| Baseline: mitigação de hoje | `verify_unchanged` perdeu a corrida 4, 2 e 0 vezes em 100k (3 execuções). `open(path)` ingênuo perdeu ~1.400 a 1.800. | Janela real, pequena |

## 2. Ambiente e ressalvas de medição

- macOS 27.0, arm64, Rust 1.99.1, `libc` 0.2.191. Máquina sob carga alta (load average 15 a 28 durante as execuções).
- Não há host Linux: o código `openat2` compila só em `cfg(target_os = "linux")` e **não foi compilado nem executado**
  (`cargo check --target x86_64-unknown-linux-gnu` falha porque o target não está instalado). Tratar como esboço.
- O "hoje" do protótipo é um modelo de `authorize_path` + `verify_unchanged` (`canonicalize` + checagem de prefixo +
  segunda `canonicalize` + `File::open` do caminho canônico). Não inclui o custo de casar o permission set, que existe
  nos dois lados. Não foi medido o `fs.readFileSync` do Velyr de ponta a ponta (não há binário release no tree sujo e a
  carga distorceria a medição); o número é o custo de open+read+close em Rust.
- Latência: mesmo processo, variantes intercaladas em rodízio (rotação por rodada), 2 rodadas de warm-up, 40 rodadas de
  2.000 operações, arquivo de 64 bytes. O valor reportado é a **mediana da razão por rodada** contra "today sync", o que
  cancela a deriva de carga entre variantes. Os ns absolutos são ruidosos; as razões são o dado confiável.
- Race: um atacante, uma vítima. Outras formas (vários atacantes, rename de diretórios com conteúdo, `cp` recursivo) não
  foram exercitadas.

## 3. Prova de conceito

### 3.1 Raiz e componentes

A raiz da allow-list é aberta uma vez, já canônica (o que `canonicalize_prefix` em
`crates/runtime-permissions/src/path.rs` produz para o grant), com `O_RDONLY | O_DIRECTORY`. O caminho relativo à raiz
passa pelo mesmo contrato léxico de `NormalizedPath::resolve`: só `Component::Normal`; `..`, `.`, absoluto e vazio são
recusados antes de qualquer syscall.

### 3.2 macOS, `openat` por componente

```rust
for (index, name) in names.iter().enumerate() {
    let last = index + 1 == names.len();
    let parent = current.as_ref().map_or(root, |fd| fd.as_raw_fd());
    let how = if last { flags } else { O_RDONLY | O_DIRECTORY };
    // SAFETY: parent is an open directory fd, c is NUL-terminated.
    let fd = unsafe { libc::openat(parent, c.as_ptr(), how | O_NOFOLLOW | O_CLOEXEC) };
    ...
}
```

### 3.3 macOS, chamada única (descoberta durante o spike)

O SDK (`sys/fcntl.h`) expõe flags que fazem o kernel aplicar a contenção numa só syscall:

- `O_NOFOLLOW_ANY` (`0x20000000`, macOS 11+): nenhum symlink em nenhum componente.
- `O_RESOLVE_BENEATH` (`0x00001000`): a resolução não pode sair do diretório do fd; symlinks que ficam abaixo são
  seguidos, absolutos e `..` que escapam falham com `ENOTCAPABLE` (107).

Chamada: `openat(root_fd, "a/b/c", O_RDONLY | O_RESOLVE_BENEATH | O_NOFOLLOW | O_CLOEXEC)`. A constante certa é `0x1000`;
a primeira tentativa (`0x10000000`, valor errado) foi ignorada pelo kernel e deu 56 escapes em 5.000 tentativas, o que
mostra que a flag precisa ser conferida contra o header e coberta por teste.

### 3.4 Linux, `openat2` (NÃO EXECUTADO)

```rust
let how = OpenHow { flags: (flags | O_CLOEXEC) as u64, mode: 0, resolve: RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS };
libc::syscall(libc::SYS_openat2, root, c.as_ptr(), &how as *const OpenHow, size_of::<OpenHow>())
```

Requer kernel 5.6+. `RESOLVE_NO_SYMLINKS` dá o efeito de `O_NOFOLLOW_ANY`; `RESOLVE_BENEATH` sozinho dá o de
`O_RESOLVE_BENEATH`. Falta: compilar, rodar o race test e medir latência num host Linux, e definir o fallback para
kernels < 5.6 (`ENOSYS`).

## 4. Race test: troca de symlink 100k vezes

Layout: `root/swap/target` (conteúdo `INSIDE`), `root/link -> ../outside`, `outside/target` (conteúdo `OUTSIDE-SECRET`).
Uma thread atacante cicla `rename` atômicos: `swap -> hold`, `link -> swap` (agora `swap` é symlink para fora),
`swap -> link`, `hold -> swap` (diretório real de volta). A vítima faz 100.000 aberturas de `swap/target` em paralelo e
classifica cada resultado pelo `(dev, ino)` do arquivo aberto: igual ao de `outside/target` conta como **escape**. Erros
de errno são classificados e contados, nenhum é engolido (errno inesperado entra em `other` e aparece no relatório).

Contagens da execução 1; a última coluna lista as 3 execuções:

| Variante | inside | ESCAPED | refused | missing (ENOENT) | Escapes nas 3 execuções |
|---|---|---|---|---|---|
| `open(path)` ingênuo | 1780 | 1499 | 0 | 96721 | 1499 / 1803 / 1432 |
| hoje: canonicalize + verify + open | 9993 | 4 | 14631 | 75346 | 4 / 2 / 0 |
| `openat` por componente | 542 | 0 | 23797 | 75661 | 0 / 0 / 0 |
| `openat` `O_NOFOLLOW_ANY` | 1422 | 0 | 19860 | 78718 | 0 / 0 / 0 |
| `openat` `O_RESOLVE_BENEATH` | 1320 | 0 | 17734 | 80946 | 0 / 0 / 0 |

(Na execução 1 a variante "hoje" também registrou 1 errno inesperado, `EINVAL` (22), vindo de `canonicalize` durante a
troca; foi reportado, não ignorado.)

Leitura:

- A mitigação atual não é inócua: sob martelada de rename ela ainda perde de 0 a 4 vezes em 100k. Isso é consistente com
  o texto do SECURITY.md (microssegundos de janela), e agora está quantificado.
- Todas as variantes handle-relative ficaram em zero. É o esperado por construção: o kernel resolve contenção e symlink
  no mesmo passo em que abre, então não há intervalo entre checar e usar.
- A maior parte das tentativas cai em `missing`: o atacante passa boa parte do ciclo com `swap` ausente entre dois
  renames. Os swaps efetivos ficaram entre ~400 (handle-relative) e ~8.000 (hoje, porque `canonicalize` demora mais e
  dá mais tempo ao atacante). Zero escapes com menos swaps é evidência mais fraca do que parece; a garantia vem do
  argumento por construção, o teste só confirma que não há surpresa.
- `ENOTCAPABLE` (107) em `O_RESOLVE_BENEATH` é o kernel recusando o symlink para fora, como desenhado.

## 5. Latência de abrir e ler (vs hoje)

Mediana da razão por rodada contra "today sync" (canonicalize + prefixo + open). Menor é melhor.

| Variante | profundidade 1 | profundidade 5 | profundidade 10 |
|---|---|---|---|
| `open(path)` simples (referência, sem checagem) | 0,52 | 0,45 | 0,40 |
| hoje, sync (canonicalize + open) | 1,00 (22,4 us) | 1,00 (28,1 us) | 1,00 (31,1 us) |
| hoje, async (canonicalize + verify + open) | 1,43 | 1,52 | 1,56 |
| `openat` por componente | 1,00 | **2,45** | **3,71** |
| `O_NOFOLLOW_ANY` (1 syscall) | 0,53 | 0,45 | 0,39 |
| `O_RESOLVE_BENEATH` (1 syscall) | 0,52 | 0,46 | 0,39 |

- Por componente custa um `openat` + `close` por nível, então cresce linear com a profundidade e estoura o teto de +10%
  já em profundidades pequenas. NO-GO para este desenho.
- As variantes de 1 syscall custam o mesmo que um `open` simples: o ganho vem de tirar o `realpath` do macOS (várias
  chamadas por componente). Passa o critério com folga, mas parte da vantagem é que "hoje" é caro, não que o handle seja
  barato. Contra um `open` sem checagem nenhuma, o overhead é ~0%.
- Linux `openat2`: não medido. Espera-se custo próximo de `open` mais a resolução do kernel, mas é palpite.

## 6. Semântica: o que cada variante aceita

Raiz com `real/f`, `inlink -> real`, `outlink -> ../outside`, `abslink -> /abs/outside`, `dangling -> nowhere`:

| Caminho relativo | por componente | `O_NOFOLLOW_ANY` | `BENEATH` + `NOFOLLOW` | só `BENEATH` |
|---|---|---|---|---|
| `real/f` | abre | abre | abre | abre |
| `inlink/f` (symlink dentro da raiz) | recusa | recusa | **abre** | **abre** |
| `outlink/f`, `abslink/f` | recusa | recusa | recusa (107) | recusa (107) |
| `dangling` | recusa | recusa | recusa | recusa (ENOENT) |
| `real/../real/f` | recusa (lexical) | recusa (lexical) | recusa (lexical) | abre |

Consequências para o Velyr:

1. **Symlinks dentro da raiz** (por exemplo `node_modules` do pnpm) hoje funcionam: `canonicalize` segue o link e o
   permission set vê o caminho final. Com `NO_SYMLINKS`/`NOFOLLOW_ANY` eles quebram. Só `BENEATH` os preserva e ainda
   garante contenção.
2. **Furo de deny com `BENEATH`**: um symlink dentro do grant apontando para um subdiretório coberto por `deny` aninhado
   abre sem passar pelo permission set, porque o caminho final nunca é canonicalizado. Hoje isso é barrado porque o
   caminho canônico é conferido contra os denies. Quem adotar `BENEATH` precisa manter a checagem do canônico (e então
   a janela volta para o deny) ou usar `NO_SYMLINKS` e aceitar a quebra do item 1.
3. **Link que sai da raiz para outra raiz permitida** (grant A contém link para grant B) hoje passa; com qualquer
   variante é recusado, porque a contenção é por raiz.

## 7. Impacto na API

Pontos em `crates/io-fs/src/sync_ops.rs` e `crates/compat-node/src/fs_ops.rs`:

- `Access` (`Read`/`Write`): não muda. Continua nomeando a permissão pedida; só ganha um mapeamento para flags de open.
- `PathMode`: `resolve()` (canonicalize) sai do caminho quente.
  - `Follow` (stat, readFile, open sem create) vira `openat` com `BENEATH` (ou `NO_SYMLINKS`).
  - `Create` (writeFile, mkdir) precisa de `O_CREAT`/`mkdirat` a partir do dirfd do pai.
  - `Entry` (lstat, readlink, unlink, rename) precisa das variantes `*at` (`fstatat` com `AT_SYMLINK_NOFOLLOW`,
    `unlinkat`, `renameat`); `AT_SYMLINK_NOFOLLOW_ANY` e `AT_RESOLVE_BENEATH` existem no macOS.
  - `Deep` (`mkdir -p`) vira um laço de `mkdirat` componente a componente.
- `FsFile(Arc<File>)`: continua servindo, o handle aberto já é um `File`. Mudam as operações que hoje recebem caminho
  (`stat`, `readdir`, `rename` com dois caminhos, `cp` e `rm` recursivos): precisam de equivalentes por fd (`fstat`,
  `fdopendir`, `renameat`, `unlinkat`) e o walk recursivo passa a descer por dirfd.
- `Guard`/`guarded`/`verify_unchanged`: ficam sem função nas operações convertidas. O job assíncrono passa a carregar o
  handle da raiz (por exemplo `Arc<OwnedFd>`) e o caminho relativo, em vez do caminho canônico.
- Ciclo de vida da raiz: uma raiz aberta uma vez fica presa ao inode. Se for substituída (rename do diretório) ou não
  existir quando o set é construído, o handle fica obsoleto ou ausente. Precisa de política (reabrir sob demanda e
  comparar `(dev, ino)`).
- Multi-raiz e `allow-read=/`: escolher a raiz mais longa que cobre o caminho; para `/` a contenção é vazia.
- `unsafe`: o workspace tem `unsafe_code = "forbid"` (`Cargo.toml`). `libc::openat` direto exige `unsafe`. Alternativa:
  `rustix` (já no `Cargo.lock`, 1.1.5 e 0.38.44, mas hoje só transitivo) oferece `openat`/`openat2` seguros no Linux;
  se expõe `O_NOFOLLOW_ANY`/`O_RESOLVE_BENEATH` no macOS não foi verificado. Qualquer rota adiciona dependência direta
  ou um crate isolado com `unsafe` permitido.
- Windows: sem equivalente direto aqui (seria `NtCreateFile` com `RootDirectory`); fora do escopo, continua só com
  `verify_unchanged`.
- Escopo do RT-22: o plano lista só `io-fs/src/{sync_ops.rs,ops.rs}` e `fs_ops.rs`. Cobrir rename, unlink e as
  árvores recursivas é bem mais que trocar o open; se o RT-22 for adiante, estimar para esse tamanho.

## 8. Como reproduzir

O projeto Cargo do spike está em
`/private/tmp/claude-501/-Users-lucasbrito-dev-wdt-velyr-core/05cffceb-5fc1-455b-a6b8-7e52d6f0259c/scratchpad/rt16/`
(efêmero, fora do repo): `cargo build --release --offline`, depois `target/release/race 100000 <scratch>`,
`target/release/latency <scratch> <depth>` e `target/release/semantics <scratch>`.

## 9. Recomendação ao Lucas

Lucas, contra os critérios do plano o resultado é misto e a decisão é sua: o race test fecha com zero escapes em todas as
variantes handle-relative (3 x 100k no macOS), enquanto o `verify_unchanged` de hoje ainda perdeu a corrida 2 a 4 vezes
em 2 de 3 execuções de 100k sob martelada, então o ganho de segurança é real mas pequeno e só relevante contra script
hostil que já roda código e controla symlinks dentro de um diretório concedido; na latência, `openat` por componente é
NO-GO (+145% a profundidade 5), mas uma única syscall com `O_RESOLVE_BENEATH` (macOS) ou `openat2(RESOLVE_BENEATH)` (Linux)
ficou 55% mais rápida que o caminho atual no macOS, passando o teto de +10%. Recomendo GO condicional: só no desenho de
syscall única, só depois de rodar race e latência num host Linux (o `openat2` não foi executado aqui), e só se você
registrar em `docs/SECURITY.md` a decisão que sobrescreve o VELYR-181, resolvendo antes o furo de deny via symlink
interno (seção 6, item 2), a necessidade de `unsafe` ou dependência nova (seção 7) e o fato de o RT-22 ser maior que um
open (rename, unlink, árvores recursivas). Se você preferir manter o isolamento no nível do SO como fronteira rígida, o
NO-GO também é defensável e mantém `verify_unchanged` sem custo; sem o seu registro em SECURITY.md o RT-22 permanece
bloqueado.
