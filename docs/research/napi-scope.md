---
title: Escopo de versão N-API e símbolos exigidos pelo corpus nativo
ticket: VELYR-95
status: research
date: 2026-10-15
related: [ADR-009, ADR-011, ADR-013]
---

# Escopo de versão N-API e símbolos exigidos pelo corpus nativo (VELYR-95)

## 1. Resumo

- Os 9 addons do corpus que são `.node` (rolldown, swc, lightningcss, sharp, better-sqlite3, bcrypt, argon2, bufferutil, utf-8-validate) somam 129 símbolos `napi_*` distintos: 95 são imports duros (`nm -u`, 8 pacotes) e 34 são nomes que só aparecem na tabela `dlsym` do rolldown. Nenhum importa `node_api_*`. esbuild é um executável Go sem nenhum símbolo N-API (controle zero).
- Pelos símbolos, o teto é N-API 8 (better-sqlite3, via `napi_type_tag_object` e `napi_check_object_type_tag`). Pela versão declarada pelo módulo, o teto é 10: better-sqlite3 e argon2 declaram 10, sharp declara 9. O Node clampa declarações abaixo de 8 para 8 e rejeita declarações acima de `NODE_API_SUPPORTED_VERSION_MAX` (10 no Node 24). O compat-napi precisa implementar os símbolos até a versão 8 e aceitar versões declaradas até 10. Aceitar só até 8 quebra 3 dos 9 addons no load, mesmo que eles não chamem nada além da v8.
- Ausência de símbolo não é benigna no macOS arm64 (testado): referência só por chamada carrega e dá SIGSEGV na primeira chamada; referência por endereço falha o `dlopen`. Daí a recomendação de exportar todos os símbolos da versão alvo, com stubs que retornam `napi_generic_failure`.
- No macOS, `-Wl,-exported_symbols_list` (testado com rustc 1.99.1 / ld64) exporta exatamente a lista, sem exportar os 1744 símbolos globais que `-export_dynamic` expõe, e sobrevive a `dead_strip` sem `#[used]`.
- Dois comportamentos fora do padrão afetam o design: rolldown resolve `napi_*` por `dlsym` (tabela de nomes, não imports), e o `.node` do swc é um "carrier" que descomprime e faz `dlopen` de um segundo addon (inferido por strings e imports, nunca executado).

## 2. Método e limites

Script: `scripts/napi-symbols.sh <workdir> [headers-ref]`. Para cada pacote roda `npm pack --ignore-scripts` (sem lifecycle scripts), extrai só o arquivo alvo do tarball e lê símbolos com `nm -u` (imports) e `nm -gU` (exports). Nada baixado é executado, carregado ou instalado. A versão mínima por símbolo vem dos guards `#if NAPI_VERSION >= N` de `js_native_api.h` e `node_api.h` (heurística de awk; 161 símbolos mapeados, 6 experimentais).

Comando executado: `scripts/napi-symbols.sh <scratchpad>/napi-research`, resultado `packages analyzed: 10/10` (9 addons mais esbuild como controle), `unique napi symbols: 129`, `non-napi host imports (uv_/v8/node): 0`. Todos os pacotes foram obtidos via `npm pack`; nenhum ficou sem binário.

Limites desta análise:

- Plataforma: somente darwin-arm64. Os conjuntos de símbolos tendem a ser os mesmos por plataforma (mesmo código-fonte), mas não foi verificado em Linux/Windows. Linux: `nm -D -u <arquivo>.node`. Windows: `dumpbin /imports <arquivo>.node` (a saída lista `node.exe` ou `node.dll` como DLL de origem; o mesmo filtro `napi_|node_api_` se aplica).
- Imports estáticos não provam uso em runtime: um símbolo importado pode nunca ser chamado, e um símbolo só existente em tabela `dlsym` pode ser opcional.
- Versões analisadas mudam no registry; os sha256 abaixo identificam os arquivos exatos.

| Pacote | Versão do tarball | Arquivo analisado | Método | Init | Versão declarada | Símbolos | sha256 do arquivo |
|---|---|---|---|---|---|---|---|
| rolldown | @rolldown/binding-darwin-arm64 1.2.14 | rolldown-binding.darwin-arm64.node | `strings` (tabela de nomes) | v1 | nenhuma | 124 | 4e4ecbcd8bbdf4b189127493f79fef124c5c3dee1a68c02b2953bf29e9aebf1f |
| swc | @swc/core-darwin-arm64 1.16.18 | swc.darwin-arm64.node (carrier) + payload zstd | nm no payload | v1 (carrier) | 3 (carrier), payload nenhuma | 45 | 478f3fc96bd210a0e6b5c37c8282e1d432401ba0fdbbe0ce73ae97f4cd67e737 |
| lightningcss | lightningcss-darwin-arm64 1.33.0 | lightningcss.darwin-arm64.node | nm | v1 | nenhuma | 55 | aee1601d7145d89f262d4c777f308ef06ed53046ee49b780d31a98f02db2e253 |
| sharp | @img/sharp-darwin-arm64 0.35.5 | sharp-darwin-arm64-0.35.5.node | nm | v1 | 9 | 53 | 791921f975371be33aed727e06283282cc001c960792eb52505b579fdaaf1b55 |
| better-sqlite3 | better-sqlite3 13.0.3 | prebuilds/darwin-arm64.node | nm | v1 | 10 | 60 | 98e0e8acd01c632fe5615243e1296af0372826f8783b18fc31c506f73c47459c |
| bcrypt | bcrypt 6.0.0 | prebuilds/darwin-arm64/bcrypt.node | nm | legado (`napi_module_register`) | n/a (efetiva 8) | 40 | 56778980f60ec6f56680928534ec894d390a9e091d43c439ccdbe9d13c88db8a |
| argon2 | argon2 0.45.1 | prebuilds/darwin-arm64/argon2.armv8.glibc.node | nm | v1 | 10 | 39 | f15eb7cc6bc02c8bbdcce4f3255f72d4feea9960d93400c70ba95a3a827d526e |
| bufferutil | bufferutil 4.1.0 | prebuilds/darwin-arm64/bufferutil.node | nm | legado | n/a (efetiva 8) | 6 | 688f957f5a989f6406e5caa26382e213c8b7316eb600303307dc95be60c287c0 |
| utf-8-validate | utf-8-validate 6.0.6 | prebuilds/darwin-arm64/utf-8-validate.node | nm | legado | n/a (efetiva 8) | 5 | 3aa78e7e2eda10bb968a98f96d713981fa6e9316efce0c750af1b3e8fe50170f |
| esbuild (controle) | @esbuild/darwin-arm64 0.28.2 | bin/esbuild (executável) | nm | nenhum | n/a | 0 | 10b6243df618d374bb2d5c9cfbe7052e1405f6aa4e53a6164f11a91b9f2e1384 |

Todos os prebuilds vêm dentro do tarball npm (better-sqlite3, bcrypt, argon2, bufferutil, utf-8-validate em `prebuilds/`; os demais em pacotes por plataforma). Não foi preciso usar tarball de GitHub release.

Notas de método:

- A "versão declarada" é a constante retornada por `node_api_module_get_api_version_v1`, lida por `otool -tV` (leitura de disassembly, sem executar). No swc foi lida do carrier; o payload não tem o getter.
- rolldown não tem nenhum import `napi_*` em `nm -u`. Ele usa `dlopen`/`dlsym` e embute os nomes como strings. Isso é o comportamento padrão do `napi-sys` 3.x (feature `dyn-symbols` habilitada por default): carrega todos os símbolos do host em `load()` via `libloading`; símbolo ausente vira stub que imprime `Node-API symbol X has not been loaded` e retorna status 1 (`napi_invalid_arg`). Os 124 nomes são um limite superior do que rolldown chama, não uma lista de imports duros. Por isso rolldown aparece como `d` na tabela e não entra na contagem de "imports duros".
- Contagem de `napi_module_register`: aparece com 4, mas só 3 são imports duros (bcrypt, bufferutil, utf-8-validate). Em rolldown é só uma string da tabela.
- Imports não-N-API do host (`uv_*`, `v8::*`, `node::*`): 0 em todos os arquivos. O contrato de host do corpus é exclusivamente Node-API (rolldown pode chamar `napi_get_uv_event_loop` via tabela, mas não há import de `uv_*`).

## 3. Versão N-API por pacote

Dois critérios independentes. O primeiro diz quais símbolos o host precisa ter. O segundo diz qual versão o host precisa aceitar no load.

| Pacote | Símbolo de maior versão (versão) | Declarada | Efetiva no Node 24 | Observação |
|---|---|---|---|---|
| rolldown | `napi_acquire_threadsafe_function` etc. (4) | nenhuma | 8 | tabela napi-sys sem símbolos de v5+ |
| swc | nenhum acima de 1 | carrier 3 | 8 | payload só usa símbolos v1 |
| lightningcss | `napi_add_finalizer` (5) | nenhuma | 8 | threadsafe functions (4) |
| sharp | `napi_add_finalizer` (5) | 9 | 9 | declara 9 sem usar símbolo de v9 |
| better-sqlite3 | `napi_type_tag_object`, `napi_check_object_type_tag` (8) | 10 | 10 | bigint (6) |
| bcrypt | nenhum acima de 1 | n/a | 8 (padrão) | init legado |
| argon2 | `napi_add_finalizer` (5) | 10 | 10 | |
| bufferutil | nenhum acima de 1 | n/a | 8 (padrão) | init legado |
| utf-8-validate | nenhum acima de 1 | n/a | 8 (padrão) | init legado |

Regras do Node 24 (fonte: `src/node_api.cc` e `src/node_version.h`, ref `v24.x`): `NODE_API_SUPPORTED_VERSION_MAX` é 10, `NODE_API_DEFAULT_MODULE_API_VERSION` é 8. Versão declarada menor que 8 vira 8. Versão maior que 10 (e diferente de `NAPI_VERSION_EXPERIMENTAL`) gera erro `requires Node-API version N, but this version of Node.js only supports version 10`. Módulos com init legado (`napi_module_register`) ficam na versão padrão 8: `napi_module_register_cb` chama `napi_module_register_by_symbol` sem passar versão, e o argumento default é `NODE_API_DEFAULT_MODULE_API_VERSION` (`src/node_binding.h`). A matriz de versões da documentação confirma v10 em Node 22.14+, 23.6+ e posteriores.

Símbolos de v9 e v10 no header (nenhum é importado pelo corpus): v9 `node_api_create_syntax_error`, `node_api_get_module_file_name`, `node_api_symbol_for`, `node_api_throw_syntax_error`; v10 `node_api_create_buffer_from_arraybuffer`, `node_api_create_external_string_latin1`, `node_api_create_external_string_utf16`, `node_api_create_property_key_latin1`, `node_api_create_property_key_utf16`, `node_api_create_property_key_utf8`.

Conclusão sobre o alvo `NAPI_VERSION 8`: está correto como superfície de símbolos a implementar. Como política de aceitação de módulo, o teto deve ser 10 (igual ao Node 24), senão sharp, better-sqlite3 e argon2 falham no load.

## 4. Tabela de símbolos

Legenda: `x` = import duro (`nm -u`); `d` = nome na tabela `dlsym` do napi-sys (rolldown, limite superior); "N-API" = versão mínima do símbolo segundo os headers do Node; "Pacotes" = número de pacotes (inclui rolldown). Ordenado por número de pacotes. Colunas: rolldown, swc, lightningcss, sharp, better-sqlite3, bcrypt, argon2, bufferutil, utf-8-validate.

| Símbolo | N-API | Pacotes | rolldown | swc | lightningcss | sharp | better-sqlite3 | bcrypt | argon2 | bufferutil | utf-8-validate |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `napi_create_function` | 1 | 9 | d | x | x | x | x | x | x | x | x |
| `napi_get_cb_info` | 1 | 9 | d | x | x | x | x | x | x | x | x |
| `napi_call_function` | 1 | 7 | d | x | x | x | x | x | x |  |  |
| `napi_create_error` | 1 | 7 | d | x | x | x | x | x | x |  |  |
| `napi_create_object` | 1 | 7 | d | x | x | x | x | x | x |  |  |
| `napi_create_reference` | 1 | 7 | d | x | x | x | x | x | x |  |  |
| `napi_create_string_utf8` | 1 | 7 | d | x | x | x | x | x | x |  |  |
| `napi_delete_reference` | 1 | 7 | d | x | x | x | x | x | x |  |  |
| `napi_get_and_clear_last_exception` | 1 | 7 | d | x | x | x | x | x | x |  |  |
| `napi_get_named_property` | 1 | 7 | d | x | x | x | x | x | x |  |  |
| `napi_get_property` | 1 | 7 | d | x | x | x | x | x | x |  |  |
| `napi_get_reference_value` | 1 | 7 | d | x | x | x | x | x | x |  |  |
| `napi_get_value_string_utf8` | 1 | 7 | d | x | x | x | x | x | x |  |  |
| `napi_is_exception_pending` | 1 | 7 | d | x | x | x | x | x | x |  |  |
| `napi_set_named_property` | 1 | 7 | d | x | x | x | x |  | x | x |  |
| `napi_throw` | 1 | 7 | d | x | x | x | x | x | x |  |  |
| `napi_typeof` | 1 | 7 | d | x | x | x | x | x | x |  |  |
| `napi_get_boolean` | 1 | 6 | d |  | x | x | x | x |  |  | x |
| `napi_get_typedarray_info` | 1 | 6 | d |  | x | x | x | x | x |  |  |
| `napi_get_undefined` | 1 | 6 | d | x | x | x | x | x |  |  |  |
| `napi_close_escapable_handle_scope` | 1 | 5 | d |  |  | x | x | x | x |  |  |
| `napi_close_handle_scope` | 1 | 5 | d |  |  | x | x | x | x |  |  |
| `napi_create_array_with_length` | 1 | 5 | d | x | x | x | x |  |  |  |  |
| `napi_create_async_work` | 1 | 5 | d | x |  | x |  | x | x |  |  |
| `napi_create_buffer_copy` | 1 | 5 | d |  | x | x | x |  | x |  |  |
| `napi_create_double` | 1 | 5 | d |  | x | x | x | x |  |  |  |
| `napi_create_type_error` | 1 | 5 | d |  |  | x | x | x | x |  |  |
| `napi_define_properties` | 1 | 5 | d |  |  | x | x | x | x |  |  |
| `napi_delete_async_work` | 1 | 5 | d | x |  | x |  | x | x |  |  |
| `napi_escape_handle` | 1 | 5 | d |  |  | x | x | x | x |  |  |
| `napi_fatal_error` | 1 | 5 | d |  |  | x | x | x | x |  |  |
| `napi_get_last_error_info` | 1 | 5 | d |  |  | x | x | x | x |  |  |
| `napi_get_null` | 1 | 5 | d | x | x | x | x |  |  |  |  |
| `napi_get_value_bool` | 1 | 5 | d | x | x | x | x |  |  |  |  |
| `napi_has_property` | 1 | 5 | d |  |  | x | x | x | x |  |  |
| `napi_open_escapable_handle_scope` | 1 | 5 | d |  |  | x | x | x | x |  |  |
| `napi_open_handle_scope` | 1 | 5 | d |  |  | x | x | x | x |  |  |
| `napi_queue_async_work` | 1 | 5 | d | x |  | x |  | x | x |  |  |
| `napi_set_element` | 1 | 5 | d | x | x | x | x |  |  |  |  |
| `napi_set_property` | 1 | 5 | d | x | x |  | x | x |  |  |  |
| `napi_add_finalizer` | 5 | 4 |  |  | x | x | x |  | x |  |  |
| `napi_create_external_buffer` | 1 | 4 | d |  | x | x | x |  |  |  |  |
| `napi_create_promise` | 1 | 4 | d | x | x |  |  |  | x |  |  |
| `napi_create_string_latin1` | 1 | 4 | d |  |  | x |  | x | x |  |  |
| `napi_define_class` | 1 | 4 | d | x | x |  | x |  |  |  |  |
| `napi_get_array_length` | 1 | 4 | d |  | x | x | x |  |  |  |  |
| `napi_get_buffer_info` | 1 | 4 | d | x |  |  |  |  |  | x | x |
| `napi_get_element` | 1 | 4 | d |  | x | x | x |  |  |  |  |
| `napi_get_global` | 1 | 4 | d | x | x |  | x |  |  |  |  |
| `napi_get_value_double` | 1 | 4 | d |  | x | x | x |  |  |  |  |
| `napi_get_value_int32` | 1 | 4 | d |  |  | x | x | x |  |  |  |
| `napi_is_buffer` | 1 | 4 | d | x |  |  | x | x |  |  |  |
| `napi_module_register` | 1 | 4 | d |  |  |  |  | x |  | x | x |
| `napi_reject_deferred` | 1 | 4 | d | x | x |  |  |  | x |  |  |
| `napi_resolve_deferred` | 1 | 4 | d | x | x |  |  |  | x |  |  |
| `napi_add_env_cleanup_hook` | 3 | 3 | d |  | x |  | x |  |  |  |  |
| `napi_coerce_to_string` | 1 | 3 | d | x | x |  |  |  |  |  |  |
| `napi_create_external` | 1 | 3 | d | x |  |  |  | x |  |  |  |
| `napi_get_property_names` | 1 | 3 | d |  | x | x |  |  |  |  |  |
| `napi_get_value_int64` | 1 | 3 | d |  |  | x |  |  |  | x |  |
| `napi_get_value_uint32` | 1 | 3 | d |  |  | x |  |  | x |  |  |
| `napi_has_named_property` | 1 | 3 | d |  | x | x |  |  |  |  |  |
| `napi_has_own_property` | 1 | 3 | d |  |  | x | x |  |  |  |  |
| `napi_is_array` | 1 | 3 | d |  | x |  | x |  |  |  |  |
| `napi_is_error` | 1 | 3 | d | x | x |  |  |  |  |  |  |
| `napi_new_instance` | 1 | 3 | d |  | x |  | x |  |  |  |  |
| `napi_reference_unref` | 1 | 3 | d | x | x |  |  |  |  |  |  |
| `napi_remove_wrap` | 1 | 3 | d | x |  |  | x |  |  |  |  |
| `napi_throw_error` | 1 | 3 | d | x | x |  |  |  |  |  |  |
| `napi_unwrap` | 1 | 3 | d | x |  |  | x |  |  |  |  |
| `napi_wrap` | 1 | 3 | d | x |  |  | x |  |  |  |  |
| `napi_adjust_external_memory` | 1 | 2 | d | x |  |  |  |  |  |  |  |
| `napi_call_threadsafe_function` | 4 | 2 | d |  | x |  |  |  |  |  |  |
| `napi_cancel_async_work` | 1 | 2 | d | x |  |  |  |  |  |  |  |
| `napi_coerce_to_number` | 1 | 2 | d |  |  |  |  |  | x |  |  |
| `napi_coerce_to_object` | 1 | 2 | d | x |  |  |  |  |  |  |  |
| `napi_create_array` | 1 | 2 | d |  |  | x |  |  |  |  |  |
| `napi_create_buffer` | 1 | 2 | d |  | x |  |  |  |  |  |  |
| `napi_create_int32` | 1 | 2 | d |  | x |  |  |  |  |  |  |
| `napi_create_range_error` | 1 | 2 | d |  |  |  | x |  |  |  |  |
| `napi_create_symbol` | 1 | 2 | d |  |  |  |  | x |  |  |  |
| `napi_create_threadsafe_function` | 4 | 2 | d |  | x |  |  |  |  |  |  |
| `napi_create_uint32` | 1 | 2 | d |  | x |  |  |  |  |  |  |
| `napi_get_new_target` | 1 | 2 | d |  |  |  | x |  |  |  |  |
| `napi_get_prototype` | 1 | 2 | d |  |  |  | x |  |  |  |  |
| `napi_get_value_external` | 1 | 2 | d | x |  |  |  |  |  |  |  |
| `napi_is_promise` | 1 | 2 | d |  | x |  |  |  |  |  |  |
| `napi_is_typedarray` | 1 | 2 | d |  | x |  |  |  |  |  |  |
| `napi_make_callback` | 1 | 2 | d |  |  | x |  |  |  |  |  |
| `napi_release_threadsafe_function` | 4 | 2 | d |  | x |  |  |  |  |  |  |
| `napi_unref_threadsafe_function` | 4 | 2 | d |  | x |  |  |  |  |  |  |
| `napi_acquire_threadsafe_function` | 4 | 1 | d |  |  |  |  |  |  |  |  |
| `napi_async_destroy` | 1 | 1 | d |  |  |  |  |  |  |  |  |
| `napi_async_init` | 1 | 1 | d |  |  |  |  |  |  |  |  |
| `napi_check_object_type_tag` | 8 | 1 |  |  |  |  | x |  |  |  |  |
| `napi_close_callback_scope` | 3 | 1 | d |  |  |  |  |  |  |  |  |
| `napi_coerce_to_bool` | 1 | 1 | d |  |  |  |  |  |  |  |  |
| `napi_create_arraybuffer` | 1 | 1 | d |  |  |  |  |  |  |  |  |
| `napi_create_bigint_int64` | 6 | 1 |  |  |  |  | x |  |  |  |  |
| `napi_create_dataview` | 1 | 1 | d |  |  |  |  |  |  |  |  |
| `napi_create_external_arraybuffer` | 1 | 1 | d |  |  |  |  |  |  |  |  |
| `napi_create_int64` | 1 | 1 | d |  |  |  |  |  |  |  |  |
| `napi_create_string_utf16` | 1 | 1 | d |  |  |  |  |  |  |  |  |
| `napi_create_typedarray` | 1 | 1 | d |  |  |  |  |  |  |  |  |
| `napi_delete_element` | 1 | 1 | d |  |  |  |  |  |  |  |  |
| `napi_delete_property` | 1 | 1 | d |  |  |  |  |  |  |  |  |
| `napi_fatal_exception` | 3 | 1 | d |  |  |  |  |  |  |  |  |
| `napi_get_arraybuffer_info` | 1 | 1 | d |  |  |  |  |  |  |  |  |
| `napi_get_dataview_info` | 1 | 1 | d |  |  |  |  |  |  |  |  |
| `napi_get_node_version` | 1 | 1 | d |  |  |  |  |  |  |  |  |
| `napi_get_threadsafe_function_context` | 4 | 1 | d |  |  |  |  |  |  |  |  |
| `napi_get_uv_event_loop` | 2 | 1 | d |  |  |  |  |  |  |  |  |
| `napi_get_value_bigint_int64` | 6 | 1 |  |  |  |  | x |  |  |  |  |
| `napi_get_value_string_latin1` | 1 | 1 | d |  |  |  |  |  |  |  |  |
| `napi_get_value_string_utf16` | 1 | 1 | d |  |  |  |  |  |  |  |  |
| `napi_get_version` | 1 | 1 | d |  |  |  |  |  |  |  |  |
| `napi_has_element` | 1 | 1 | d |  |  |  |  |  |  |  |  |
| `napi_instanceof` | 1 | 1 | d |  |  |  |  |  |  |  |  |
| `napi_is_arraybuffer` | 1 | 1 | d |  |  |  |  |  |  |  |  |
| `napi_is_dataview` | 1 | 1 | d |  |  |  |  |  |  |  |  |
| `napi_open_callback_scope` | 3 | 1 | d |  |  |  |  |  |  |  |  |
| `napi_ref_threadsafe_function` | 4 | 1 | d |  |  |  |  |  |  |  |  |
| `napi_reference_ref` | 1 | 1 | d |  |  |  |  |  |  |  |  |
| `napi_remove_env_cleanup_hook` | 3 | 1 | d |  |  |  |  |  |  |  |  |
| `napi_run_script` | 1 | 1 | d |  |  |  |  |  |  |  |  |
| `napi_strict_equals` | 1 | 1 | d |  |  |  |  |  |  |  |  |
| `napi_throw_range_error` | 1 | 1 | d |  |  |  |  |  |  |  |  |
| `napi_throw_type_error` | 1 | 1 | d |  |  |  |  |  |  |  |  |
| `napi_type_tag_object` | 8 | 1 |  |  |  |  | x |  |  |  |  |

Distribuição: 38 símbolos aparecem em 1 pacote só, 91 em 2 ou mais, 71 em 3 ou mais.

## 5. Decisões recomendadas

### 5.1 Lista mínima de símbolos para a v1 do compat-napi

Cortar por contagem de pacotes é errado: símbolos de um pacote só (bigint e type tag do better-sqlite3) quebram o load daquele pacote. O critério correto é a união dos imports duros dos 8 pacotes analisáveis por `nm`.

**Camada P0 (implementação real na v1): 95 símbolos**, união dos imports duros de swc (payload), lightningcss, sharp, better-sqlite3, bcrypt, argon2, bufferutil e utf-8-validate:

```
napi_add_env_cleanup_hook napi_add_finalizer napi_adjust_external_memory napi_call_function
napi_call_threadsafe_function napi_cancel_async_work napi_check_object_type_tag
napi_close_escapable_handle_scope napi_close_handle_scope napi_coerce_to_number
napi_coerce_to_object napi_coerce_to_string napi_create_array napi_create_array_with_length
napi_create_async_work napi_create_bigint_int64 napi_create_buffer napi_create_buffer_copy
napi_create_double napi_create_error napi_create_external napi_create_external_buffer
napi_create_function napi_create_int32 napi_create_object napi_create_promise
napi_create_range_error napi_create_reference napi_create_string_latin1 napi_create_string_utf8
napi_create_symbol napi_create_threadsafe_function napi_create_type_error napi_create_uint32
napi_define_class napi_define_properties napi_delete_async_work napi_delete_reference
napi_escape_handle napi_fatal_error napi_get_and_clear_last_exception napi_get_array_length
napi_get_boolean napi_get_buffer_info napi_get_cb_info napi_get_element napi_get_global
napi_get_last_error_info napi_get_named_property napi_get_new_target napi_get_null
napi_get_property napi_get_property_names napi_get_prototype napi_get_reference_value
napi_get_typedarray_info napi_get_undefined napi_get_value_bigint_int64 napi_get_value_bool
napi_get_value_double napi_get_value_external napi_get_value_int32 napi_get_value_int64
napi_get_value_string_utf8 napi_get_value_uint32 napi_has_named_property napi_has_own_property
napi_has_property napi_is_array napi_is_buffer napi_is_error napi_is_exception_pending
napi_is_promise napi_is_typedarray napi_make_callback napi_module_register napi_new_instance
napi_open_escapable_handle_scope napi_open_handle_scope napi_queue_async_work napi_reference_unref
napi_reject_deferred napi_release_threadsafe_function napi_remove_wrap napi_resolve_deferred
napi_set_element napi_set_named_property napi_set_property napi_throw napi_throw_error
napi_type_tag_object napi_typeof napi_unref_threadsafe_function napi_unwrap napi_wrap
```

Além dos símbolos, o protocolo de init faz parte do P0: (a) `napi_register_module_v1` via `dlsym` após o `dlopen`; (b) `node_api_module_get_api_version_v1` via `dlsym`, opcional, padrão 8; (c) `napi_module_register` chamado de static constructor durante o `dlopen` (bcrypt, bufferutil, utf-8-validate), o que exige capturar o `napi_module*` num slot pendente lido logo após o `dlopen` retornar.

**Camada P1 (34 símbolos exclusivos da tabela do rolldown)**: implementação real somente se rolldown for alvo de aceitação. Como o napi-sys tolera símbolo ausente (stub com status 1), rolldown carrega mesmo com P1 em stub; só falha quando chamar uma função de P1. Inclui `napi_get_uv_event_loop`, que exige expor um loop libuv-compatível; não há nenhum `uv_*` importado no corpus.

```
napi_acquire_threadsafe_function napi_async_destroy napi_async_init napi_close_callback_scope
napi_coerce_to_bool napi_create_arraybuffer napi_create_dataview napi_create_external_arraybuffer
napi_create_int64 napi_create_string_utf16 napi_create_typedarray napi_delete_element
napi_delete_property napi_fatal_exception napi_get_arraybuffer_info napi_get_dataview_info
napi_get_node_version napi_get_threadsafe_function_context napi_get_uv_event_loop
napi_get_value_string_latin1 napi_get_value_string_utf16 napi_get_version napi_has_element
napi_instanceof napi_is_arraybuffer napi_is_dataview napi_open_callback_scope
napi_ref_threadsafe_function napi_reference_ref napi_remove_env_cleanup_hook napi_run_script
napi_strict_equals napi_throw_range_error napi_throw_type_error
```

`napi_get_version` deve retornar 10 (mesmo teto do Node 24).

**Restante da superfície (stub, ver 5.2):** todos os demais símbolos não experimentais até a versão 10.

### 5.2 Comportamento de símbolo ausente

Evidência (macOS arm64, teste próprio com clang, sem código baixado): um addon com `-undefined dynamic_lookup` que só chama `napi_missing_fn`: `dlopen` com `RTLD_LAZY` e `RTLD_NOW` carrega, e a primeira chamada termina em SIGSEGV (exit 139). Um addon que toma o endereço da função (ponteiro em dado): `dlopen` falha com `symbol not found in flat namespace '_napi_missing_fn'`, em `RTLD_LAZY` e `RTLD_NOW`. Linux e Windows não foram testados; em ELF, imports de função são lazy via PLT a menos que o binário use bind-now (`-z now`), e o `napi-sys` carrega por `dlsym`. Em todos os casos "símbolo ausente" é crash ou falha de load, nunca um erro tratável pelo addon.

Recomendação:

1. Exportar todos os símbolos não experimentais de versão até 10 (145 até a v8, mais 10 de v9 e v10; os 6 experimentais ficam de fora e módulos com `NAPI_VERSION_EXPERIMENTAL` são rejeitados no load, como o Node faz).
2. Símbolo não implementado: retorna `napi_generic_failure`, grava no `napi_env` o último erro com `error_code = napi_generic_failure` e `error_message = "velyr: <symbol> is not implemented"` (ponteiro para string estática, válido depois da chamada), e emite o log estruturado `velyr.napi.unsupported` com o nome do símbolo e o módulo (uma vez por símbolo e módulo, para não inundar). `napi_get_last_error_info` devolve essa mensagem.
3. Exceções por assinatura: `napi_fatal_error` (noreturn) e `napi_module_register` (void) são P0 e sempre reais.
4. Nunca falhar o `dlopen` por símbolo faltante: o addon precisa conseguir carregar e tratar o erro, e o log dá a lista exata do que falta.
5. Teste de CI: comparar os exports do binário `velyr` com a lista derivada dos headers (o inverso do script: `nm -gU` precisa conter todos os símbolos esperados).
6. Segurança: stub e símbolos reais ficam no crate `compat-napi`, que já está na allowlist de `unsafe` (ADR-011); cada `unsafe` com `// SAFETY:`. O carregamento fica atrás de `--allow-native` (ADR-013), que `-A` nunca concede.

### 5.3 Estratégia de export dos símbolos pelo binário `velyr`

**macOS (testado).** Toolchain: rustc 1.99.1, ld64 27037.1, arm64, perfil release com `-Wl,-dead_strip` padrão do rustc. Crate `compat` (rlib) com `#[no_mangle] pub extern "C"`, binário `host` dependendo dela. Medido com `dyld_info -exports`:

| Configuração | Símbolos de teste na export trie | Total na export trie |
|---|---|---|
| sem flags, não referenciado | 0 | 273 (símbolos Rust internos) |
| sem flags, com tabela `#[used]` referenciando os símbolos | presentes | 276 |
| `-Wl,-export_dynamic` | presentes | 1744 |
| `-Wl,-exported_symbols_list,<lista>` | presentes, mesmo sem referência e sem `#[used]` | 2 (somente a lista) |

Observação nesta toolchain, sem generalizar para o ld64: o executável Rust já exporta 273 símbolos mesmo sem flags, e os símbolos de teste só ficam fora quando o rlib não é referenciado e o `dead_strip` os remove. Ou seja, o que decide a presença é a retenção pelo linker; a lista explícita é o que garante retenção e export juntos e de forma determinística (linha `exported_symbols_list`).

Recomendação: `-Wl,-exported_symbols_list,<arquivo>` gerado a partir da lista de símbolos (prefixo `_`), passado por `cargo:rustc-link-arg-bins=-Wl,-exported_symbols_list,...` num `build.rs` do binário `velyr`. Exporta exatamente a superfície N-API, não vaza os 1744 globais, e sobrevive ao `dead_strip`. Que addons com `-undefined dynamic_lookup` (padrão do `napi-build` no macOS) resolvam esses símbolos no executável é inferido da export trie e do comportamento de namespace plano; o binding ponta a ponta com o executável Rust não foi testado. O próprio Node usa `-Wl,-export_dynamic` somente no build com LTO, para evitar que o LTO remova globais que plugins precisam (comentário de `man ld` em `node.gyp`, bloco `enable_lto=="true"`).

**Linux (conhecimento prévio, sem fonte consultada nesta pesquisa, não testado).** O Node usa `-rdynamic` (`common.gypi`, bloco `OS in "linux freebsd openbsd solaris android ..."`). Para o `velyr`, preferir exportar só a superfície: `-Wl,--dynamic-list=<arquivo>` ou `-Wl,--export-dynamic-symbol=napi_*` / `node_api_*` (GNU ld e lld; versões mínimas não verificadas, confirmar no CI). Verificar com `nm -D --defined-only velyr`. Pendências a testar: se `--gc-sections` preserva símbolos de rlib não referenciada (se não, usar uma tabela `#[used]` com os ponteiros de função), e se addons com bind-now falham no `dlopen` quando falta símbolo (tende a isso).

**Windows (hook e `dllexport` verificados nas fontes; `/DEF` e `/EXPORT` são conhecimento prévio, não testados).** O Node exporta com `NAPI_EXTERN` = `__declspec(dllexport)` quando compila o próprio Node (`js_native_api.h`); addons veem `dllimport` via `node_api.h` com `BUILDING_NODE_EXTENSION`. Para o `velyr.exe`: arquivo `.def` com `EXPORTS` listando todos os símbolos, passado com `cargo:rustc-link-arg-bins=/DEF:napi.def` (ou `/EXPORT:` por símbolo). Addons MSVC linkam com `node.lib`, que importa de `node.exe`; o node-gyp injeta `win_delay_load_hook.cc`, que no evento `dliNotePreLoadLibrary` devolve o handle de `libnode.dll` se carregado, senão `GetModuleHandle(NULL)`, o que funciona com o executável renomeado (a compilação define `HOST_BINARY`). Portanto: addons feitos com node-gyp e hook funcionam com `velyr.exe` (não distribuir um `libnode.dll`). Addons do napi-rs em MSVC usam sempre `dyn-symbols` (`target_env = "msvc"` no `napi-sys`), resolvendo via `libloading` no processo, também compatível. Addons linkados direto em `node.lib` sem o hook não funcionam fora de `node.exe`; ficam fora do escopo da v1 e devem ser documentados como limitação.

## 6. Achados adicionais

- **swc é um carrier.** `swc.darwin-arm64.node` (9 MB) contém uma seção `__TEXT,__swc_native` com um payload zstd (magic `SWCN`+`ZSTD`) que decodifica para um Mach-O de 24 MB, este sim o addon com os 45 imports N-API. O carrier importa `dlopen`, `dlsym`, `mmap`, `rename`, `unlink`, `write` e tem strings sobre cache de materialização (`SWC_NATIVE_BINDING_CACHE`, `Library/Caches`) e sobre "raw addon registration resolved back to the carrier". Inferência por strings e imports; o carrier nunca foi executado. Consequência: `--allow-native=paths` protege só o `dlopen` do carrier; o segundo `dlopen` acontece dentro do código nativo e escapa ao loader do Velyr. Código nativo não é confinável pelo modelo de permissões; isso deve constar no ADR/SECURITY como limite explícito. O payload exporta `napi_register_module_v1`, que o carrier chama.
- **esbuild** não usa N-API (binário Go). Entra apenas como controle: 0 símbolos.
- **Handles e `napi_env`** são tipos do host. ADR-009 proíbe tipos V8 fora de `engine-v8` e exige handles opacos no `engine-api`; o compat-napi deve implementar handle scopes sobre essa fronteira. Fora do escopo desta pesquisa, mas é o maior custo de implementação do P0.

## 7. Não verificado

- Linux e Windows: símbolos (só darwin-arm64 analisado), comportamento de símbolo ausente e export de símbolos.
- Chamadas reais em runtime: imports estáticos não provam uso.
- Comportamento do carrier do swc em execução.
- Flags de Linux (`--dynamic-list`, `--export-dynamic-symbol`) e de Windows (`/DEF`, `/EXPORT`): sem fonte consultada, confirmar na documentação do GNU ld/lld e da Microsoft e no CI, incluindo versões mínimas.
- Binding ponta a ponta de um addon `dynamic_lookup` contra o executável Rust no macOS.
- Mapeamento símbolo para versão é heurístico (awk sobre `#if`); amostra conferida: `napi_add_finalizer` 5, threadsafe functions 4, bigint 6, type tag 8, cleanup hook 3, `napi_get_uv_event_loop` 2.

## 8. Fontes

Acessadas em 2026-10-15. Refs que se movem foram fixadas por SHA no momento do acesso.

- Node-API, documentação (`doc/api/n-api.md`, `nodejs/node` ref `v24.x`, commit `0d10bfe3cb961ef997490f5c060bd1466b5affea`): matriz de versões, `napi_extended_error_info`. https://nodejs.org/api/n-api.html
- `src/node_api.cc`, `src/node_version.h`, `src/node_binding.h` (`NODE_API_SUPPORTED_VERSION_MAX` 10, `NODE_API_DEFAULT_MODULE_API_VERSION` 8), `src/js_native_api.h`, `src/node_api.h`, `node.gyp`, `common.gypi`: mesmo commit `v24.x`. https://github.com/nodejs/node/tree/v24.x
- node-gyp `src/win_delay_load_hook.cc`: `nodejs/node-gyp` main, commit `c915679036cacf1d6192a78fb41cca5f50eae304`.
- napi-rs: `crates/sys/src/lib.rs` e `Cargo.toml` (`dyn-symbols` por default, stub com mensagem e status 1), `crates/build/src/lib.rs` (`-undefined dynamic_lookup` no macOS): `napi-rs/napi-rs` main, commit `57a539a3d2daff8fcadad20565a5af31db8d949c`. https://github.com/napi-rs/napi-rs
- Testes locais (scratchpad, não versionados): export de símbolos em Rust no macOS e `dlopen` com símbolo ausente.
