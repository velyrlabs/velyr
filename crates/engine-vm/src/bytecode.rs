//! Register bytecode with an implicit accumulator (Ignition style), produced by the
//! engine-native compiler and executed by the bytecode VM.
//!
//! Conventions:
//! - Most instructions read and/or write the accumulator (`acc`). Binary operators compute
//!   `acc = reg <op> acc`, so the left operand lives in the register.
//! - Registers `r0..parameter_count` receive the call arguments (missing ones are `undefined`);
//!   every other register starts as `undefined`.
//! - `Target` operands are instruction offsets inside the same `FunctionCode`.
//! - Context slots start as the hole (TDZ marker); `ThrowIfHole` turns a hole read into a
//!   `ReferenceError`.
//! - `handlers` are ordered innermost first: on a throw at offset `o`, the VM picks the first
//!   entry with `start <= o < end`, restores the saved context, puts the exception in `acc`
//!   and jumps to `target`.

mod function_code;

pub use function_code::{
    Constant, EvalCode, FunctionCode, GlobalDeclarations, HandlerEntry, LazyBody, SourcePosition,
    TemplateObject,
};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Reg(pub u32);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConstIndex(pub u32);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Target(pub u32);

/// `count` consecutive registers starting at `first`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct RegRange {
    pub first: Reg,
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Instruction {
    LdaUndefined,
    LdaNull,
    LdaTrue,
    LdaFalse,
    /// The TDZ marker of an uninitialized lexical binding.
    LdaTheHole,
    LdaSmi(i32),
    LdaConstant(ConstIndex),
    Ldar(Reg),
    Star(Reg),
    Mov {
        from: Reg,
        to: Reg,
    },
    /// The frame's `this`; arrow closures capture it when created.
    LdaThis,
    /// The running closure (binding of a named function expression).
    LdaCurrentClosure,
    /// `new.target`; arrows see the one of their enclosing function.
    LdaNewTarget,

    /// GlobalDeclarationInstantiation for the `GlobalDeclarations` constant.
    DeclareGlobals(ConstIndex),
    /// Name lookup on the global environment; ReferenceError if missing or in TDZ.
    LdaGlobal(ConstIndex),
    /// Like `LdaGlobal` but yields `undefined` for a missing name (`typeof x`).
    LdaGlobalInsideTypeof(ConstIndex),
    /// PutValue on a global name: strict code throws for a missing name, `const` throws.
    StaGlobal(ConstIndex),
    InitializeGlobalLexical(ConstIndex),
    /// Annex B.3.2.2: sloppy `[[Set]]` of the accumulator on a global name, skipped when shadowed by a lexical.
    InitializeAnnexBGlobal(ConstIndex),
    /// Sloppy `delete name`.
    DeleteGlobal(ConstIndex),

    /// `acc` = new context with `n` hole slots whose parent is the current context.
    CreateContext(u32),
    /// `reg` = current context; current context = `acc`.
    PushContext(Reg),
    /// Current context = `reg`.
    PopContext(Reg),
    /// `reg` = current context.
    SaveContext(Reg),
    LdaContextSlot {
        depth: u32,
        slot: u32,
    },
    StaContextSlot {
        depth: u32,
        slot: u32,
    },
    /// `CreateContext` with a `ContextNames` table; `var_scope`: target of direct-eval `var`s.
    CreateContextWithNames {
        count: u32,
        names: ConstIndex,
        var_scope: bool,
    },
    /// `acc` = new `with` context over ToObject(`reg`) whose parent is the current context.
    PushWithContext(Reg),
    /// `acc` = dynamic lookup of a name through `with` and eval-var contexts, then the globals.
    LdaLookup(ConstIndex),
    /// Like `LdaLookup` but yields `undefined` for an unresolvable name (`typeof x`).
    LdaLookupForTypeof(ConstIndex),
    /// PutValue of `acc` on a dynamically resolved name (value preserved).
    StaLookup {
        name: ConstIndex,
        strict: bool,
    },
    /// Sloppy `delete name` on a dynamically resolved name.
    DeleteLookup(ConstIndex),
    /// `acc` = the callee named `name`; `receiver` = its `with` object, or `undefined`.
    LdaLookupCallee {
        name: ConstIndex,
        receiver: Reg,
    },
    /// `base` = the resolved binding of a name (an encoded `Reference`), read and written by
    /// `LdaLookupIn` and `StaLookupIn`: an assignment target resolves before its value.
    ResolveLookup {
        name: ConstIndex,
        base: Reg,
    },
    LdaLookupIn {
        name: ConstIndex,
        base: Reg,
    },
    StaLookupIn {
        name: ConstIndex,
        base: Reg,
        strict: bool,
    },
    /// EvalDeclarationInstantiation of one `var` (or function) name in the nearest var scope.
    DeclareEvalVar {
        name: ConstIndex,
        function: bool,
    },

    /// ReferenceError `<name> is not initialized` when `acc` is the hole.
    ThrowIfHole(ConstIndex),
    /// TypeError `Assignment to constant variable '<name>'`.
    ThrowConstAssignment(ConstIndex),

    Add(Reg),
    Sub(Reg),
    Mul(Reg),
    Div(Reg),
    Mod(Reg),
    Exp(Reg),
    BitAnd(Reg),
    BitOr(Reg),
    BitXor(Reg),
    Shl(Reg),
    /// `>>`
    Sar(Reg),
    /// `>>>`
    Shr(Reg),
    TestEqual(Reg),
    TestNotEqual(Reg),
    TestEqualStrict(Reg),
    TestNotEqualStrict(Reg),
    TestLessThan(Reg),
    TestGreaterThan(Reg),
    TestLessThanOrEqual(Reg),
    TestGreaterThanOrEqual(Reg),
    /// `reg in acc`
    TestIn(Reg),
    /// `reg instanceof acc`
    TestInstanceOf(Reg),

    Negate,
    BitNot,
    LogicalNot,
    TypeOf,
    ToNumber,
    ToNumeric,
    /// Increment of the numeric value of `acc` (Number or BigInt).
    Inc,
    Dec,
    ToString,
    ToPropertyKey,
    /// TypeError when `acc` is `null`/`undefined` (destructuring source).
    CheckObjectCoercible,

    GetNamed {
        object: Reg,
        name: ConstIndex,
        /// Inline cache of the site; compilers number the sites 0.. (see `cache_mut`).
        cache: u32,
    },
    /// Key in `acc`.
    GetKeyed {
        object: Reg,
    },
    /// Value in `acc` (preserved).
    SetNamed {
        object: Reg,
        name: ConstIndex,
        /// Inline cache of the site, like `GetNamed::cache`.
        cache: u32,
    },
    /// Value in `acc` (preserved).
    SetKeyed {
        object: Reg,
        key: Reg,
    },
    /// Key in `acc`; `acc` = boolean result (strict code throws on failure).
    DeleteProperty {
        object: Reg,
    },

    CreateObject,
    CreateArray,
    /// CreateDataProperty with the value in `acc`.
    DefineNamedOwn {
        object: Reg,
        name: ConstIndex,
    },
    /// CreateDataProperty with the value in `acc`; optionally names an anonymous function.
    DefineKeyedOwn {
        object: Reg,
        key: Reg,
        set_function_name: bool,
    },
    /// Accessor function in `acc`; names it `get <key>` (`set <key>` for setters).
    DefineGetter {
        object: Reg,
        key: Reg,
    },
    DefineSetter {
        object: Reg,
        key: Reg,
    },
    /// `__proto__: acc` in an object literal.
    SetLiteralPrototype {
        object: Reg,
    },
    /// CopyDataProperties(target, acc, excluded keys).
    CopyDataProperties {
        target: Reg,
        excluded: RegRange,
    },
    /// Appends `acc` at index `array.length`.
    ArrayPush {
        array: Reg,
    },
    /// Grows `array.length` by one, leaving a hole.
    ArrayPushHole {
        array: Reg,
    },
    /// Iterates `acc` and appends every value.
    SpreadIntoArray {
        array: Reg,
    },
    CreateRegExp {
        pattern: ConstIndex,
        flags: ConstIndex,
    },
    /// Cached per call site (constant identity).
    GetTemplateObject(ConstIndex),
    /// Closure over the current context for the `Function` constant.
    CreateClosure(ConstIndex),
    CreateUnmappedArguments,
    MapArgument(u32, u32), // (argument index, context slot): see `Interpreter::map_argument`
    /// Array of the arguments from index `n` on.
    CreateRestParameter(u32),
    /// Class constructor for the `Function` constant, with the heritage read from `reg` (the
    /// hole without `extends`); `reg` = constructor, `reg+1` = prototype, `acc` = constructor.
    CreateClass(ConstIndex, Reg),
    /// [[HomeObject]] of the closure in `acc` = `reg`, for `super` property access.
    SetHomeObject(Reg),
    /// Defines the class element of `kind` (`ClassField` value, or `Method`/`Getter`/`Setter`
    /// closure, in `acc`) on object `r0` at key `r1`; private keys use private semantics.
    DefineElement(Reg, Reg, FunctionKind),
    /// Records an instance element on class `r0` (key `r1`, closure in `acc`), defined on each
    /// instance at construction; a `ClassField` closure is called for the field value.
    AddInstanceElement(Reg, Reg, FunctionKind),
    /// SetFunctionName of the anonymous function in `acc` from the property key in `reg`.
    NameFunction(Reg),
    /// `acc` = a fresh private name (a symbol only class code can reach).
    CreatePrivateName,
    /// `super(...args)` with the argument array in `reg`; `acc` = the bound `this`.
    SuperCall(Reg),
    /// `acc` = `super[acc]`.
    GetSuper,
    /// `super[reg] = acc` (value preserved).
    SetSuper(Reg),
    /// `acc` = the `[[GetPrototypeOf]]` of the running method's home object (GetSuperBase).
    LdaSuperBase,
    /// `acc` = `reg[acc]` with `this` as receiver; `reg` holds a `LdaSuperBase` result.
    GetSuperFrom(Reg),
    /// `base[key] = acc` with `this` as receiver (value preserved); `base` is a `LdaSuperBase` result.
    SetSuperFrom {
        base: Reg,
        key: Reg,
    },

    Call {
        callee: Reg,
        this: Reg,
        args: RegRange,
    },
    CallUndefinedReceiver {
        callee: Reg,
        args: RegRange,
    },
    /// A call named `eval`: when `callee` is %eval%, a direct eval through the `EvalSite`
    /// constant `site`; otherwise `CallUndefinedReceiver`.
    CallDirectEval {
        callee: Reg,
        args: RegRange,
        site: ConstIndex,
    },
    /// `args` holds an array of the arguments.
    CallSpread {
        callee: Reg,
        this: Reg,
        args: Reg,
    },
    Construct {
        callee: Reg,
        args: RegRange,
    },
    ConstructSpread {
        callee: Reg,
        args: Reg,
    },

    Jump(Target),
    /// ToBoolean(acc)
    JumpIfTrue(Target),
    JumpIfFalse(Target),
    JumpIfNullish(Target),
    JumpIfNotNullish(Target),
    JumpIfUndefined(Target),
    JumpIfNotUndefined(Target),
    Return,
    Throw,
    /// Rethrows `acc` keeping the original stack.
    ReThrow,

    /// GetIterator(acc): `record` = iterator, `record+1` = next method, `record+2` = done flag.
    GetIterator {
        record: Reg,
    },
    /// IteratorStep: `acc` = value, or sets the done flag and jumps to `done`. A throwing
    /// `next` also sets the done flag.
    IteratorStep {
        record: Reg,
        done: Target,
    },
    /// IteratorClose with a normal completion; no-op when the done flag is set.
    IteratorClose {
        record: Reg,
    },
    /// IteratorClose with a throw completion: errors from `return` are ignored.
    IteratorCloseAfterThrow {
        record: Reg,
    },
    /// Key enumerator over `acc` (`null`/`undefined` enumerate nothing).
    ForInPrepare {
        enumerator: Reg,
    },
    /// `acc` = next key, or jumps to `done`.
    ForInNext {
        enumerator: Reg,
        done: Target,
    },

    /// Prologue of a generator or async function (VELYR-109): `generator` = a new generator
    /// object owning the frame. Generators suspend here and return that object; async
    /// functions keep running and the call returns their promise at the first `Await`.
    CreateGenerator {
        generator: Reg,
        kind: CoroutineKind,
    },
    /// Saves the frame in its generator and leaves it with `acc` (yielded or awaited). On
    /// resumption `acc` = the sent value and `mode` = its `ResumeMode` as a number.
    Suspend {
        mode: Reg,
        kind: SuspendKind,
    },
    /// After a `Suspend`: a throw resumption throws `acc`, a return one jumps to `on_return`.
    Resume {
        mode: Reg,
        on_return: Target,
    },
    /// Catch-all handler of an async function: rejects its promise with `acc`, returns it.
    AsyncFunctionReject,
    /// GetIterator(acc, async) into the same 3-register record as `GetIterator`.
    GetAsyncIterator {
        record: Reg,
    },
    /// `acc` = Call(next, iterator), unchecked; the done flag stays set until
    /// `IteratorUnpack` sees a result that is not done.
    IteratorNext {
        record: Reg,
    },
    /// Checks the iterator result in `acc`: done sets the flag and jumps to `done`; either
    /// way `acc` = its value.
    IteratorUnpack {
        record: Reg,
        done: Target,
    },
    /// One `yield*` step: calls next/throw/return of the delegate with `acc` per `mode`;
    /// `acc` = the unchecked result. A missing `return` jumps to `on_return` with `acc`
    /// unchanged; a missing `throw` closes the delegate and throws a TypeError.
    YieldStarCall {
        record: Reg,
        mode: Reg,
        on_return: Target,
    },
    /// After `YieldStarCall` (and its await in async generators): checks the result in
    /// `acc`; done jumps to `done` with `acc` = its value. Otherwise async generators get
    /// `acc` = its value and generators keep the result object, yielded as is.
    YieldStarUnpack {
        record: Reg,
        done: Target,
    },
    /// AsyncIteratorClose, first half: jumps to `done` when the done flag is set or there
    /// is no `return` method, else `acc` = Call(return, iterator), to be awaited.
    AsyncIteratorClose {
        record: Reg,
        done: Target,
    },
    /// Throws a TypeError unless `acc` is an object (an awaited `return` result).
    CheckIteratorResult,
    /// `acc` = an imported binding: slots `slot` and `slot + 1` of the context at `depth`
    /// hold the exporting module's context and the binding's slot in it (set by linking).
    LdaModuleImport {
        depth: u32,
        slot: u32,
    },
    /// `import(acc, options)`: `acc` = a promise for the namespace, loaded by the host hook.
    DynamicImport {
        options: Option<Reg>,
    },
    /// `reg` = a new, empty dispose stack (explicit resource management).
    CreateDisposeStack(Reg),
    /// AddDisposableResource(`acc`), hint async-dispose when `is_async`.
    AddDisposable {
        stack: Reg,
        is_async: bool,
    },
    /// Pops the last resource and calls its dispose method, `acc` = the result (`undefined`
    /// for a nullish `await using` value); jumps to `done` when the stack is empty.
    DisposeNext {
        stack: Reg,
        done: Target,
    },
    /// `error` = `acc` when it holds the hole, else SuppressedError(`acc`, `error`).
    DisposeRecordError {
        error: Reg,
    },
}

impl Instruction {
    pub fn target_mut(&mut self) -> Option<&mut Target> {
        match self {
            Instruction::Jump(target)
            | Instruction::JumpIfTrue(target)
            | Instruction::JumpIfFalse(target)
            | Instruction::JumpIfNullish(target)
            | Instruction::JumpIfNotNullish(target)
            | Instruction::JumpIfUndefined(target)
            | Instruction::JumpIfNotUndefined(target)
            | Instruction::IteratorStep { done: target, .. }
            | Instruction::ForInNext { done: target, .. }
            | Instruction::Resume {
                on_return: target, ..
            }
            | Instruction::IteratorUnpack { done: target, .. }
            | Instruction::YieldStarUnpack { done: target, .. }
            | Instruction::AsyncIteratorClose { done: target, .. }
            | Instruction::DisposeNext { done: target, .. }
            | Instruction::YieldStarCall {
                on_return: target, ..
            } => Some(target),
            _ => None,
        }
    }

    /// The inline cache index of a named property access.
    pub fn cache_mut(&mut self) -> Option<&mut u32> {
        match self {
            Instruction::GetNamed { cache, .. } | Instruction::SetNamed { cache, .. } => {
                Some(cache)
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionKind {
    Script,
    Normal,
    Arrow,
    Method,
    Getter,
    Setter,
    ClassConstructor,
    /// Returning from it checks the late-bound `this` (set by `super()`).
    DerivedConstructor,
    /// Field initializer: called with the instance as `this`, returns the field value.
    ClassField,
    /// Direct eval code: shares `this`, `new.target` and the home object of its caller.
    Eval,
}

/// What a `CreateGenerator` prologue makes of its function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoroutineKind {
    Generator,
    Async,
    AsyncGenerator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuspendKind {
    Yield,
    Await,
    /// `yield*` in a generator: `acc` is the delegate's iterator result, returned as is.
    Delegate,
}

/// How a suspended generator is resumed (`next`, `throw`, `return`), stored as a number in
/// the `mode` register of `Suspend`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeMode {
    Next = 0,
    Throw = 1,
    Return = 2,
}
