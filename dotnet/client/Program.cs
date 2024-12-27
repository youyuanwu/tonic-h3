using Helloworld;
using Grpc.Net.Client;
using System.Net;

// disable server cert validation
var httpClientHandler = new HttpClientHandler
{
    ServerCertificateCustomValidationCallback = HttpClientHandler.DangerousAcceptAnyServerCertificateValidator,
    SslProtocols = System.Security.Authentication.SslProtocols.Tls13
};

var handler = new Http3Handler(httpClientHandler);

var httpClient = new HttpClient(handler)
{
    DefaultRequestVersion = HttpVersion.Version30,
    DefaultVersionPolicy = HttpVersionPolicy.RequestVersionExact,
};

var channel = GrpcChannel.ForAddress("https://127.0.0.1:5047", new GrpcChannelOptions()
{
    HttpClient = httpClient
});

var client = new Greeter.GreeterClient(channel);

var response = await client.SayHelloAsync(
    new HelloRequest { Name = "World" });

Console.WriteLine(response.Message);


/// <summary>
/// A delegating handler that changes the request HTTP version to HTTP/3.
/// From: https://github.com/dotnet/AspNetCore.Docs/blob/c55fd5f6f0a41bb61548f346d01cbf1084fe1a73/aspnetcore/grpc/troubleshoot/sample/8.0/GrpcGreeterClient/Program.cs#L189
/// </summary>
public class Http3Handler : DelegatingHandler
{
    public Http3Handler() { }
    public Http3Handler(HttpMessageHandler innerHandler) : base(innerHandler) { }

    protected override Task<HttpResponseMessage> SendAsync(
        HttpRequestMessage request, CancellationToken cancellationToken)
    {
        request.Version = HttpVersion.Version30;
        request.VersionPolicy = HttpVersionPolicy.RequestVersionExact;

        return base.SendAsync(request, cancellationToken);
    }
}