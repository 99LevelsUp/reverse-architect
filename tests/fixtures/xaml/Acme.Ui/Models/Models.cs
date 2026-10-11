namespace Acme.Models
{
    public class ModelB
    {
        public string P { get; set; }
        public string Name { get; set; }
        public Holder Holder { get; set; }
        public string Title { get; set; }
        public string Caption { get; set; }

        public void Refresh()
        {
            OnPropertyChanged("Title");
            OnPropertyChanged(nameof(Caption));
            OnPropertyChanged("Unknown_Value");
        }

        void OnPropertyChanged(string name) { }
    }

    public class ModelC
    {
        public string P { get; set; }
    }

    public class ModelD
    {
        public string Q { get; set; }
    }

    public class ModelE
    {
        public string P { get; set; }
    }

    public class ModelF
    {
        public string R { get; set; }
    }

    public class ModelG
    {
        public string R { get; set; }
    }

    public class ItemModel
    {
        public string Name { get; set; }
    }

    public class Holder
    {
        public object Command { get; set; }
    }

    public class Logger
    {
        public void Touch()
        {
            Write("Title");
        }

        void Write(string text) { }
    }
}
