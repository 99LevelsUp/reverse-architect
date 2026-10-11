//! The editor of the acceptance examples of step 2b (AE1–AE10), as one
//! repository: a view that reaches its view model through the base list of a
//! generic base, a command class bound from its own code that raises an event,
//! the view model's check with its three silent returns and the properties it
//! writes, an external caller of the check, and a save that clears the check's
//! state before it reads it. U5 adds a menu item of the view given the check
//! command, a command whose key gesture (`Ctrl+F4`) is assigned in C#, and the
//! new editor: a view and a view model with the same names of command and key,
//! whose `CanCheck()` returns `false` — an independent component, never to be
//! mixed with the old one (AE2, AE3, AE5).
//!
//! Loaded with `#[path = "common/editor_fixture.rs"] mod editor_fixture;` by the
//! test crates that use it, so `tests/common/mod.rs` stays as it is. Units U5–U9
//! of step 2b extend it: a new file or a new member is added here, and the
//! tests that read it keep their meaning.

#![allow(dead_code)]

/// The view, its generic base and the save.
pub const VIEW: &str = r#"using Acme.ViewModels;

namespace Acme.Views
{
    public class ViewBase<T>
    {
        protected T viewModel;
    }

    public class EditorView : ViewBase<EditorViewModel>
    {
        public EditorView()
        {
            PreviewKeyDown += Editor_PreviewKeyDown;
            viewModel.CheckCommand.Executed += OnChecked;
        }

        void Editor_PreviewKeyDown(object sender, KeyEventArgs e)
        {
            if (e.Key == Key.F4 && (viewModel.ShowErrors || viewModel.CanErrors))
            {
                this.viewModel.CheckCommand.Execute(null);
            }
        }

        void OnChecked(object sender, EventArgs e) { Store(); }

        public void Save()
        {
            if (!BeforeSave()) return;
            Store();
        }

        bool BeforeSave()
        {
            viewModel.CheckWasDone = false;
            viewModel.CheckCommand.Execute(null);
            return viewModel.CheckWasDone;
        }

        void Store()
        {
            Db.Apply("UPDATE dbo.Documents SET Body = @body");
        }

        void BuildMenu()
        {
            var check = new MenuItem();
            check.Header = "Check errors";
            check.Command = viewModel.CheckCommand;
        }
    }
}
"#;

/// The new editor's view: the same key and the same command name as the old
/// one, over its own view model.
pub const VIEW_NEW: &str = r#"using Acme.ViewModels;

namespace Acme.Views
{
    public class EditorViewNew : ViewBase<EditorViewModelNew>
    {
        public EditorViewNew()
        {
            PreviewKeyDown += Editor_PreviewKeyDown;
        }

        void Editor_PreviewKeyDown(object sender, KeyEventArgs e)
        {
            if (e.Key == Key.F4)
            {
                this.viewModel.CheckCommand.Execute(null);
            }
        }
    }
}
"#;

/// The new editor's view model: its check can never run.
pub const VIEW_MODEL_NEW: &str = r#"namespace Acme.ViewModels
{
    public class EditorViewModelNew
    {
        private RelayCommand checkCommand;

        public bool Checked { get; set; }

        public RelayCommand CheckCommand
        {
            get
            {
                if (checkCommand == null) checkCommand = new RelayCommand(p => Check(), p => CanCheck());
                return checkCommand;
            }
        }

        public void Check() { Checked = true; }

        bool CanCheck() { return false; }
    }
}
"#;

/// The command class, the view model and the external check.
pub const VIEW_MODEL: &str = r#"using Acme.Parsing;

namespace Acme.ViewModels
{
    public enum SyntaxKind { Sql, Script, Text }

    public class RelayCommand
    {
        private readonly Action<object> _execute;
        private readonly Predicate<object> _canExecute;
        public event EventHandler Executed;
        public event CancelEventHandler ExecutingStarted;
        public KeyGesture[] KeyGestures { get; set; }
        public RelayCommand(Action<object> execute) : this(execute, null) { }
        public RelayCommand(Action<object> execute, Predicate<object> canExecute)
        {
            _execute = execute;
            _canExecute = canExecute;
        }
        public bool CanExecute(object parameter) { return _canExecute == null || _canExecute(parameter); }
        public void Execute(object parameter)
        {
            var evt = new CancelEventArgs();
            ExecutingStarted?.Invoke(this, evt);
            if (evt.Cancel) return;
            _execute(parameter);
            Executed?.Invoke(this, EventArgs.Empty);
        }
    }

    public class EditorViewModel
    {
        private RelayCommand checkCommand;
        private RelayCommand closeErrorsCommand;
        private readonly Parser parser = new Parser();
        private bool canErrors;
        private SyntaxKind syntax;
        private List<string> errors;
        private bool showErrors;
        private string messageErrors;
        private int selectedIndex;

        public string Text { get; set; }
        public bool CheckWasDone { get; set; }
        public bool CanErrors => canErrors;

        public List<string> Errors
        {
            get { return errors; }
            set { errors = value; OnPropertyChanged(); }
        }

        public bool ShowErrors
        {
            get { return showErrors; }
            set { showErrors = value; OnPropertyChanged(); }
        }

        public string MessageErrors
        {
            get { return messageErrors; }
            set { messageErrors = value; OnPropertyChanged(); }
        }

        public int SelectedIndex
        {
            get { return selectedIndex; }
            set { selectedIndex = value; OnPropertyChanged(); }
        }

        public RelayCommand CheckCommand
        {
            get
            {
                if (checkCommand == null) checkCommand = new RelayCommand(p => Check(), p => (syntax == SyntaxKind.Sql || syntax == SyntaxKind.Script) && canErrors);
                return checkCommand;
            }
        }

        public bool ErrorsPanelOpen { get; set; }

        public RelayCommand CloseErrorsCommand
        {
            get
            {
                if (closeErrorsCommand == null)
                {
                    closeErrorsCommand = new RelayCommand(p => CloseErrors());
                    closeErrorsCommand.KeyGestures = new KeyGesture[] { new KeyGesture(Key.F4, ModifierKeys.Control) };
                }
                return closeErrorsCommand;
            }
        }

        void CloseErrors() { ErrorsPanelOpen = false; }

        public void Check() { Check((Keyboard.Modifiers & ModifierKeys.Shift) == ModifierKeys.Shift); }

        public void Check(bool syntaxOnly)
        {
            CheckWasDone = false;
            if (!canErrors) return;
            if (syntax != SyntaxKind.Sql && syntax != SyntaxKind.Script) return;
            if (Text == null) return;
            var result = parser.Parse(Text, syntaxOnly);
            Errors = result.Errors;
            ShowErrors = Errors.Count > 0;
            MessageErrors = result.Message;
            SelectedIndex = 0;
            CheckWasDone = true;
        }

        void Refresh()
        {
            if (!CheckWasDone) return;
            SelectedIndex = 1;
        }

        void OnPropertyChanged() { }
    }

    internal class ExternalCheck
    {
        private EditorViewModel editor;

        public bool Run()
        {
            editor.Check(false);
            return editor.CheckWasDone;
        }
    }
}
"#;

/// The parser of another project: one step of a flow, never expanded.
pub const PARSER: &str = r#"namespace Acme.Parsing
{
    public class ParseResult
    {
        public List<string> Errors;
        public string Message;
    }

    public class Parser
    {
        public ParseResult Parse(string text, bool syntaxOnly) { return new ParseResult(); }
    }
}
"#;

pub const VIEWS_NS: &str = "sym:csharp:Acme.Views#";
pub const MODELS_NS: &str = "sym:csharp:Acme.ViewModels#";
pub const PARSING_NS: &str = "sym:csharp:Acme.Parsing#";

/// The repository's files, by path.
pub fn files() -> Vec<(&'static str, &'static str)> {
    vec![
        ("src/Parsing/Parser.cs", PARSER),
        ("src/ViewModels/EditorViewModel.cs", VIEW_MODEL),
        ("src/ViewModels/EditorViewModelNew.cs", VIEW_MODEL_NEW),
        ("src/Views/EditorView.cs", VIEW),
        ("src/Views/EditorViewNew.cs", VIEW_NEW),
    ]
}
