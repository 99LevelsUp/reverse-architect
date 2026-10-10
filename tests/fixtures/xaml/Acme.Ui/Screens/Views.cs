using Acme.Models;

namespace Acme.Screens
{
    /// The base of every view; its type parameter is the view's model.
    public class ViewBase<TViewModel>
    {
        public object GetTemplateChild(string name) { return null; }
        public object FindResource(string key) { return null; }
    }

    /// A view on a generic base, styled by a template in each look.
    public class ViewA : ViewBase<ModelB>
    {
        public bool IsBusy { get; set; }

        public void Attach()
        {
            object list = GetTemplateChild("PART_List");
            object none = GetTemplateChild("PART_None");
            object brush = FindResource("MarkerBrush");
            object ink = FindResource("MotifInk");
            object nothing = FindResource("NoSuchKey");
        }
    }

    /// A view that holds its model in a field instead of a generic base.
    public class PanelD
    {
        private ModelD viewModel;
    }

    /// A view whose model is not known.
    public class Bare
    {
    }

    /// A view with code-behind.
    public class ViewE : ViewBase<ModelE>
    {
        private Saver saver;
        void OnSave(object sender, object e) { saver.Store(); }
    }

    /// What the view's handler leads to: nothing in C# calls the handler.
    public class Saver
    {
        public void Store() { }
    }
}

namespace Acme.A
{
    public class ViewA : Acme.Screens.ViewBase<ModelF>
    {
    }
}

namespace Acme.B
{
    public class ViewA : Acme.Screens.ViewBase<ModelG>
    {
    }
}

namespace Acme
{
    public class Settings
    {
        public static Settings Default;
    }
}

namespace Acme.Behaviors
{
    public static class Focus
    {
        public static bool GetIsInitial(object target) { return false; }
        public static void SetIsInitial(object target, bool value) { }
    }
}
