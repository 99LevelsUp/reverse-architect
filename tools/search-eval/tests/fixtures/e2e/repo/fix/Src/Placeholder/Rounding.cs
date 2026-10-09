namespace Placeholder
{
    public static class Rounding
    {
        public static decimal Apply(decimal value) => value;

        public static decimal Apply(decimal value, int digits) => decimal.Round(value, digits);
    }
}
