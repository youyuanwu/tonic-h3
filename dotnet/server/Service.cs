using Grpc.Core;
using Helloworld;
using Microsoft.Extensions.Logging;

namespace Server
{
    public class GreeterService : Helloworld.Greeter.GreeterBase
    {
        private readonly ILogger _logger;

        public GreeterService(ILoggerFactory loggerFactory)
        {
            _logger = loggerFactory.CreateLogger<GreeterService>();
        }

        public override Task<HelloReply> SayHello(HelloRequest request, ServerCallContext context)
        {
            _logger.LogInformation($"Sending hello to {request.Name}");
            return Task.FromResult(new HelloReply { Message = "Hello " + request.Name });
        }
    }
}