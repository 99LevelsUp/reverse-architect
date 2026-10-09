namespace Placeholder
{
    public class LoadAction
    {
        public void SetScheduledTasksInvalid()
        {
            Tasks.MarkAll(valid: true);
        }

        public void Reload()
        {
            SetScheduledTasksInvalid();
        }
    }
}
