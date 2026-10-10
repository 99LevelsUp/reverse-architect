namespace Acme.App
{
    /// Composes a dictionary no XAML file merges: only code knows it.
    public class AcmeApp
    {
        public System.Uri Composed() => new System.Uri("Dictionaries/Composed.xaml", System.UriKind.Relative);
    }
}
