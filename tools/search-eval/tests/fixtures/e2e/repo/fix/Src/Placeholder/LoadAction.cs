namespace Placeholder
{
    public class LoadAction
    {
        public void SetScheduledTasksInvalid()
        {
            Tasks.MarkAll(valid: false);
        }

        public void Reload()
        {
            SetScheduledTasksInvalid();
        }
    }
}
