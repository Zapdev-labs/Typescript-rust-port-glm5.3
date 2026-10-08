// Minimal port of tsc/internal/ast/diagnostic.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// This module carries only the `Diagnostic` struct + its accessors/setters
// (what the SourceFile fields reference). The full diagnostic.go surface —
// Localize (locale crate), displayMessageArgs (spanmap), the diagnostic
// chain factories, and the RepopulateDiagnostic machinery beyond the struct —
// is a separate M2 task.
//
// Go name mapping:
//   Diagnostic struct            → Diagnostic (private fields + Go accessors)
//   RepopulateDiagnosticKind/Info → same names
//   file *SourceFile             → Option<NodeId> (SPEC §5.1 handle)

use tsc_core::text::TextRange;

use crate::NodeId;

/// Go: `type RepopulateDiagnosticKind int` + consts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(i32)]
pub enum RepopulateDiagnosticKind {
    /// Go: `RepopulateModeMismatch RepopulateDiagnosticKind = 1`.
    Mismatch = 1,
    /// Go: `RepopulateModuleNotFound RepopulateDiagnosticKind = 2`.
    ModuleNotFound = 2,
}

/// Go: `type RepopulateDiagnosticInfo struct`.
#[derive(Clone, Debug, Default)]
pub struct RepopulateDiagnosticInfo {
    pub kind: RepopulateDiagnosticKind,
    pub module_reference: String,
    pub mode: tsc_core::compileroptions::ResolutionMode,
    pub package_name: String,
}

/// Go: `type Diagnostic struct` — the parser/binder diagnostic carrier.
#[derive(Clone, Debug)]
pub struct Diagnostic {
    file: Option<NodeId>,
    loc: TextRange,
    code: i32,
    category: tsc_diagnostics::Category,
    /// Go: non-empty marks the diagnostic as coming from an external source.
    source: String,
    /// Original message; may be None.
    message: Option<&'static tsc_diagnostics::Message>,
    /// Already-localized message used when `message` is None.
    message_text: String,
    message_key: &'static str,
    message_args: Vec<String>,
    message_chain: Vec<Diagnostic>,
    related_information: Vec<Diagnostic>,
    reports_unnecessary: bool,
    reports_deprecated: bool,
    skipped_on_no_emit: bool,
    repopulate_info: Option<RepopulateDiagnosticInfo>,
}

impl Diagnostic {
    /// Go: the parser/binder constructors (`ast.NewDiagnostic`-shaped); the
    /// localized-message path.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        file: Option<NodeId>,
        loc: TextRange,
        code: i32,
        category: tsc_diagnostics::Category,
        source: impl Into<String>,
        message: Option<&'static tsc_diagnostics::Message>,
        message_text: impl Into<String>,
        message_key: impl Into<&'static str>,
        message_args: Vec<String>,
    ) -> Self {
        Diagnostic {
            file,
            loc,
            code,
            category,
            source: source.into(),
            message,
            message_text: message_text.into(),
            message_key: message_key.into(),
            message_args,
            message_chain: Vec::new(),
            related_information: Vec::new(),
            reports_unnecessary: message.is_some_and(|m| m.reports_unnecessary()),
            reports_deprecated: false,
            skipped_on_no_emit: false,
            repopulate_info: None,
        }
    }

    /// Go: `func (d *Diagnostic) File() *SourceFile`.
    pub fn file(&self) -> Option<NodeId> {
        self.file
    }
    /// Go: `func (d *Diagnostic) Pos() int`.
    pub fn pos(&self) -> tsc_core::text::TextPos {
        self.loc.pos()
    }
    /// Go: `func (d *Diagnostic) End() int`.
    pub fn end(&self) -> tsc_core::text::TextPos {
        self.loc.end()
    }
    /// Go: `func (d *Diagnostic) Len() int`.
    pub fn len(&self) -> tsc_core::text::TextPos {
        self.loc.len()
    }
    /// Go: `func (d *Diagnostic) Loc() core.TextRange`.
    pub fn loc(&self) -> TextRange {
        self.loc
    }
    /// Go: `func (d *Diagnostic) Code() int32`.
    pub fn code(&self) -> i32 {
        self.code
    }
    /// Go: `func (d *Diagnostic) Category() diagnostics.Category`.
    pub fn category(&self) -> tsc_diagnostics::Category {
        self.category
    }
    /// Go: `func (d *Diagnostic) Source() string`.
    pub fn source(&self) -> &str {
        &self.source
    }
    /// Go: `func (d *Diagnostic) MessageText() string`.
    pub fn message_text(&self) -> &str {
        &self.message_text
    }
    /// Go: `func (d *Diagnostic) MessageKey() diagnostics.Key`.
    pub fn message_key(&self) -> &'static str {
        self.message_key
    }
    /// Go: `func (d *Diagnostic) MessageArgs() []string`.
    pub fn message_args(&self) -> &[String] {
        &self.message_args
    }
    /// Go: `func (d *Diagnostic) MessageChain() []*Diagnostic`.
    pub fn message_chain(&self) -> &[Diagnostic] {
        &self.message_chain
    }
    /// Go: `func (d *Diagnostic) RelatedInformation() []*Diagnostic`.
    pub fn related_information(&self) -> &[Diagnostic] {
        &self.related_information
    }
    /// Go: `func (d *Diagnostic) ReportsUnnecessary() bool`.
    pub fn reports_unnecessary(&self) -> bool {
        self.reports_unnecessary
    }
    /// Go: `func (d *Diagnostic) ReportsDeprecated() bool`.
    pub fn reports_deprecated(&self) -> bool {
        self.reports_deprecated
    }
    /// Go: `func (d *Diagnostic) SkippedOnNoEmit() bool`.
    pub fn skipped_on_no_emit(&self) -> bool {
        self.skipped_on_no_emit
    }
    /// Go: `func (d *Diagnostic) RepopulateInfo() *RepopulateDiagnosticInfo`.
    pub fn repopulate_info(&self) -> Option<&RepopulateDiagnosticInfo> {
        self.repopulate_info.as_ref()
    }

    /// Go: `func (d *Diagnostic) SetFile(file *SourceFile)`.
    pub fn set_file(&mut self, file: Option<NodeId>) {
        self.file = file;
    }
    /// Go: `func (d *Diagnostic) SetLocation(loc core.TextRange)`.
    pub fn set_location(&mut self, loc: TextRange) {
        self.loc = loc;
    }
    /// Go: `func (d *Diagnostic) SetCategory(category diagnostics.Category)`.
    pub fn set_category(&mut self, category: tsc_diagnostics::Category) {
        self.category = category;
    }
    /// Go: `func (d *Diagnostic) SetSkippedOnNoEmit()`.
    pub fn set_skipped_on_no_emit(&mut self) {
        self.skipped_on_no_emit = true;
    }
    /// Go: `func (d *Diagnostic) SetRepopulateInfo(info *RepopulateDiagnosticInfo)`.
    pub fn set_repopulate_info(&mut self, info: Option<RepopulateDiagnosticInfo>) {
        self.repopulate_info = info;
    }

    /// Go: `func (d *Diagnostic) SetExternalData(source string, messageText string) *Diagnostic`.
    pub fn set_external_data(&mut self, source: impl Into<String>, message_text: impl Into<String>) -> &mut Self {
        self.source = source.into();
        self.message_text = message_text.into();
        self
    }

    /// Go: `func (d *Diagnostic) SetMessageChain(messageChain []*Diagnostic) *Diagnostic`.
    pub fn set_message_chain(&mut self, message_chain: Vec<Diagnostic>) -> &mut Self {
        self.message_chain = message_chain;
        self
    }

    /// Go: `func (d *Diagnostic) AddMessageChain(messageChain *Diagnostic) *Diagnostic`.
    pub fn add_message_chain(&mut self, message_chain: Diagnostic) -> &mut Self {
        self.message_chain.push(message_chain);
        self
    }

    /// Go: `func (d *Diagnostic) SetRelatedInfo(relatedInformation []*Diagnostic) *Diagnostic`.
    pub fn set_related_info(&mut self, related_information: Vec<Diagnostic>) -> &mut Self {
        self.related_information = related_information;
        self
    }

    /// Go: `func (d *Diagnostic) AddRelatedInfo(relatedInformation *Diagnostic) *Diagnostic`.
    pub fn add_related_info(&mut self, related_information: Diagnostic) -> &mut Self {
        self.related_information.push(related_information);
        self
    }
}
