//! The editor of the acceptance examples of step 2b (AE1–AE10), as one
//! repository: a view that reaches its view model through the base list of a
//! generic base, a command class bound from its own code that raises an event,
//! the view model's check with its three silent returns and the properties it
//! writes, an external caller of the check, and a save that clears the check's
//! state before it reads it.
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
        ("src/Views/EditorView.cs", VIEW),
    ]
}
