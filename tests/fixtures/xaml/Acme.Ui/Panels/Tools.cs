using System;
using Acme.Screens;

namespace Acme.Tools
{
    /// A command that runs the delegate it is built with.
    public class RelayCommand
    {
        private readonly Action<object> _execute;
        public RelayCommand(Action<object> execute) { _execute = execute; }
        public bool CanExecute(object parameter) { return true; }
        public void Execute(object parameter) { _execute?.Invoke(parameter); }
    }

    /// A menu item of the repository's own, on WPF's.
    public class AcmeMenuItem : MenuItem
    {
    }

    public interface IReloader
    {
        void Reload();
    }

    /// The model of the panels: its commands are what their controls, menu
    /// items and keys run.
    public class ModelH : IReloader
    {
        public RelayCommand SaveCommand => new RelayCommand(p => Persist());
        public RelayCommand OpenCommand => new RelayCommand(p => Fetch());
        public string Label { get; set; }

        void Persist() { }
        void Fetch() { }

        public void Reload() { }
        public virtual void Reset() { }
        public void OnClosed(object sender, EventArgs e) { }
    }

    /// A view styled by a template in each panel look.
    public class ViewH : ViewBase<ModelH>
    {
    }

    /// A view with code-behind and keys of its own.
    public class ViewK : ViewBase<ModelH>
    {
    }
}
